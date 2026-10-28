//! Being a polite caller: bounded concurrency, a request-rate cap, and a bounded wait-and-retry for transient
//! failures. Nothing here is about avoiding detection. A bot or login challenge is never retried, the
//! `User-Agent` never changes and no other client identity is ever tried: see `docs/DESIGN.md`.

use crate::Error;
use crate::transport::{BoxFuture, Request, Response, Transport};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Monotonic time the politeness machinery runs on, in milliseconds. Only deltas are used, so the origin is
/// arbitrary. Injectable so tests never actually wait.
pub trait Clock: Send + Sync + 'static {
    /// Millis since this clock's own origin. Must never go backwards.
    fn now_ms(&self) -> u64;

    /// Wait `ms`. Cancel-safe, like [`Transport::send`].
    fn sleep_ms(&self, ms: u64) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if ms > 0 {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
        })
    }
}

/// The production [`Clock`]: `Instant` for ticks, `tokio::time::sleep` for waits.
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    base: &'static Instant,
}

/// A [`Clock`] for tests: time only moves when something asks to sleep, so a retry schedule that would cost a
/// minute of real waiting costs none. Sharing one between concurrent calls makes each caller's waits advance
/// every other's clock too, which is fine for assertions about *that* call.
#[derive(Default, Debug)]
pub struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Milliseconds this clock has passed: the total simulated waiting.
    pub fn elapsed_ms(&self) -> u64 {
        self.now.load(Ordering::Relaxed)
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.elapsed_ms()
    }

    fn sleep_ms(&self, ms: u64) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.now.fetch_add(ms, Ordering::Relaxed);
        })
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TokioClock {
    pub fn new() -> Self {
        static BASE: OnceLock<Instant> = OnceLock::new();
        Self { base: BASE.get_or_init(Instant::now) }
    }
}

impl Clock for TokioClock {
    fn now_ms(&self) -> u64 {
        self.base.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
    }
}

/// A bijection on `u64` (splitmix64's finaliser), used to turn a counter into a scattered value.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A deterministic spread in `0..span_ms` from `raw`. Concurrent clients that hit the same rate limit wake at
/// slightly different moments instead of in lockstep, which is the only reason it exists. It does not hide
/// anything: the request itself is unchanged.
pub fn spread(raw: u64, span_ms: u64) -> u64 {
    if span_ms == 0 { 0 } else { splitmix64(raw) % span_ms }
}

/// The limits one [`Client`](crate::Client) obeys. All of them are ceilings: a call that never hits a rate limit
/// waits nothing and sends one request at a time as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Total sends per call, including the first. `1` disables retrying.
    pub attempts: u8,
    /// The wait before the second send, doubling for each later one.
    pub base_delay: Duration,
    /// Ceiling for one wait this crate computes itself. A `Retry-After` the service sent is not capped by it: that
    /// wait is kept as stated, and [`Policy::total_budget`] deciding to stop instead.
    pub max_delay: Duration,
    /// Ceiling for the whole call: sending, waiting and reading included.
    pub total_budget: Duration,
    /// Sends in flight at once, for the whole client.
    pub max_concurrent: usize,
    /// Sent-per-second cap, for the whole client.
    pub requests_per_second: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self::polite()
    }
}

impl Policy {
    /// The defaults: enough restraint that walking a whole library is not a flood, enough patience that a
    /// transient 429 or 5xx does not fail the call.
    pub const fn polite() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            total_budget: Duration::from_secs(60),
            max_concurrent: 4,
            requests_per_second: 2,
        }
    }

    /// Send nothing ever: no retry, no wait. The transport still gets the concurrency and rate caps.
    pub const fn no_retry() -> Self {
        Self { attempts: 1, ..Self::polite() }
    }

    /// The wait before send number `attempt` (1-based), with `jitter_ms` folded in.
    ///
    /// A `Retry-After` the service sent **is** the wait, exactly: it is not replaced by the ladder, not shortened to
    /// `max_delay` and given no spread either, because a wait the service named is not ours to guess at. Only the
    /// ladder this crate computes for itself is capped and spread. If a stated wait does not fit the remaining
    /// [`total_budget`](Self::total_budget), the retrying stops and the `RateLimited` error is returned instead of
    /// sending early; the caller then waits the seconds the service asked for.
    pub fn delay_for(&self, attempt: u8, retry_after: Option<Duration>, jitter_ms: u64) -> Duration {
        if let Some(asked) = retry_after {
            return asked;
        }
        let steps = 1u64 << u32::from(attempt.saturating_sub(1).min(8));
        let base = self.base_delay.saturating_mul(steps as u32).min(self.max_delay);
        // Spread inside a quarter of the wait, but never past the ceiling.
        let span = self.max_delay.saturating_sub(base).min(base / 4 + Duration::from_millis(1)).as_millis() as u64;
        base + Duration::from_millis(spread(jitter_ms, span))
    }

    /// Rejects settings that would deadlock or spin: zero attempts, a zero rate, a ceiling below the first step.
    pub fn validate(&self) -> Result<(), crate::Error> {
        use crate::Error;
        let ok = self.attempts >= 1
            && self.attempts <= 8
            && self.max_concurrent >= 1
            && self.max_concurrent <= 64
            && self.requests_per_second >= 1
            && self.requests_per_second <= 50
            && !self.base_delay.is_zero()
            && self.base_delay <= self.max_delay
            && self.max_delay <= self.total_budget
            && self.total_budget <= Duration::from_secs(3600);
        ok.then_some(()).ok_or(Error::InvalidRequest)
    }
}

/// A leaky bucket: `capacity` tokens, refilled continuously at `requests_per_second`.
pub(crate) struct TokenBucket {
    capacity: f64,
    refill_per_ms: f64,
    tokens: f64,
    last_ms: u64,
}

impl TokenBucket {
    fn new(requests_per_second: u32) -> Self {
        let rate = requests_per_second as f64;
        Self { capacity: rate, refill_per_ms: rate / 1000.0, tokens: rate, last_ms: 0 }
    }

    /// Take one token at `now_ms`: `0` means send now, otherwise the millis to wait before asking again.
    fn take(&mut self, now_ms: u64) -> u64 {
        self.tokens =
            (self.tokens + (now_ms.saturating_sub(self.last_ms) as f64) * self.refill_per_ms).min(self.capacity);
        self.last_ms = now_ms.max(self.last_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            0
        } else {
            ((1.0 - self.tokens) / self.refill_per_ms).ceil() as u64 + 1
        }
    }
}

/// The client's send gate: at most `max_concurrent` sends in flight, at most `requests_per_second` started.
pub(crate) struct Gate {
    permits: tokio::sync::Semaphore,
    rate: Mutex<TokenBucket>,
    policy: Policy,
    clock: Arc<dyn Clock>,
    seq: AtomicU64,
}

/// Holds one concurrency permit. Drop it before waiting for a retry, so a backoff never parks a free slot.
pub(crate) struct GateGuard<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl Gate {
    pub(crate) fn new(policy: Policy, clock: Arc<dyn Clock>) -> Self {
        Self {
            permits: tokio::sync::Semaphore::new(policy.max_concurrent),
            rate: Mutex::new(TokenBucket::new(policy.requests_per_second)),
            policy,
            clock,
            seq: AtomicU64::new(0),
        }
    }

    pub(crate) fn policy(&self) -> Policy {
        self.policy
    }

    /// Wait until the rate cap allows a send, then take a concurrency permit.
    ///
    /// The permit is taken last and held for one send only: a `GateGuard` must be dropped before any backoff
    /// sleep, so a call waiting out a `Retry-After` does not block the other sends it could still be making.
    pub(crate) async fn acquire(&self) -> GateGuard<'_> {
        loop {
            let wait = self.rate.lock().unwrap_or_else(|e| e.into_inner()).take(self.clock.now_ms());
            if wait == 0 {
                break;
            }
            self.clock.sleep_ms(wait).await;
        }
        let permit = self.permits.acquire().await.expect("the semaphore is never closed");
        GateGuard { _permit: permit }
    }

    /// The backoff before send number `attempt`.
    pub(crate) fn delay(&self, attempt: u8, retry_after: Option<Duration>) -> Duration {
        let raw = self.seq.fetch_add(1, Ordering::Relaxed) ^ self.clock.now_ms();
        self.policy.delay_for(attempt, retry_after, raw)
    }

    /// Send `attempt` through the gate, waiting and resending while the failure is transient.
    ///
    /// A send is retried only when [`Error::is_retryable`] says so, which excludes every sign-in, refusal and
    /// challenge: a bot check is reported, never hammered. Concurrency permits are released before any wait, so
    /// a call parked on a `Retry-After` does not occupy a slot. A wait that would overshoot the budget is not
    /// truncated into an early send: the retrying stops and the error is returned.
    pub(crate) async fn dispatch(&self, attempt: Attempt, timeout: Duration) -> Result<Response, Error> {
        self.dispatch_up_to(attempt, timeout, self.policy.attempts).await
    }

    /// One send, never repeated, for an account mutation: after a timeout or `5xx` the first attempt may already have
    /// taken effect, so resending it blindly could apply it twice. The caller reports the error and decides.
    pub(crate) async fn dispatch_once(&self, attempt: Attempt, timeout: Duration) -> Result<Response, Error> {
        self.dispatch_up_to(attempt, timeout, 1).await
    }

    async fn dispatch_up_to(&self, attempt: Attempt, timeout: Duration, max_sends: u8) -> Result<Response, Error> {
        let policy = self.policy;
        let start = self.clock.now_ms();
        let budget = policy.total_budget.as_millis() as u64;
        let mut sends: u8 = 0;
        loop {
            sends += 1;
            let outcome = {
                let _guard = self.acquire().await;
                tokio::time::timeout(timeout, attempt.send()).await
            };
            let error = match outcome {
                Ok(Ok(response)) => {
                    if (200..300).contains(&response.status) {
                        return Ok(response);
                    }
                    Error::classify(response.status, response.retry_after, &response.body)
                }
                Ok(Err(error)) => error,
                Err(_) => Error::Timeout,
            };
            if !error.is_retryable() || sends >= max_sends {
                return Err(error);
            }
            let spent = self.clock.now_ms().saturating_sub(start);
            if spent >= budget {
                return Err(error);
            }
            let wait = self.delay(sends + 1, error.retry_after());
            // Never send earlier than the service asked. A wait that does not fit the remaining budget ends the
            // retrying, and the error still carries the `Retry-After` for the caller to honour itself.
            if wait > Duration::from_millis(budget - spent) {
                return Err(error);
            }
            self.clock.sleep_ms(wait.as_millis() as u64).await;
        }
    }
}

/// One sendable thing, repeated by [`Gate::dispatch`] after a wait. Everything in it is owned, which is why it
/// can simply be sent again.
pub(crate) enum Attempt {
    Post { transport: Arc<dyn Transport>, request: Request },
    Get { transport: Arc<dyn Transport>, url: String },
}

impl Attempt {
    async fn send(&self) -> Result<Response, Error> {
        match self {
            Attempt::Post { transport, request } => transport.send(request.clone()).await,
            Attempt::Get { transport, url } => transport.get(url.clone()).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Policy = Policy {
        attempts: 4,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(10),
        total_budget: Duration::from_secs(60),
        max_concurrent: 2,
        requests_per_second: 5,
    };

    /// Exact ladder steps, ignoring the spread: what the wait is never shorter than.
    fn ladder(attempt: u8) -> Duration {
        P.base_delay * 2u32.pow(u32::from(attempt.saturating_sub(1)).min(4))
    }

    #[test]
    fn delay_grows_exponentially() {
        for attempt in 1..=4 {
            let step = ladder(attempt);
            let d = P.delay_for(attempt, None, attempt as u64);
            assert!(d >= step, "attempt {attempt}: {d:?} shorter than its {step:?} step");
            assert!(d <= step + step / 4, "attempt {attempt}: {d:?} spread past a quarter of its step");
        }
        // Saturating: a huge attempt number never overflows, it caps at the ceiling.
        for raw in [0, 1, 999] {
            assert_eq!(P.delay_for(40, None, raw), P.max_delay);
        }
    }

    #[test]
    fn a_stated_retry_after_is_the_wait_exactly() {
        assert_eq!(P.delay_for(1, Some(Duration::from_secs(3)), 0), Duration::from_secs(3));
        assert_eq!(P.delay_for(1, Some(Duration::from_secs(3)), 7), Duration::from_secs(3));
        // Longer than max_delay: kept, not shortened. Sending early is the worse mistake than waiting long.
        assert_eq!(P.delay_for(1, Some(Duration::from_secs(3600)), 7), Duration::from_secs(3600));
        // The exponential ladder is only for when the service asked for nothing.
        assert_eq!(P.delay_for(9, Some(Duration::from_millis(1)), 3), Duration::from_millis(1));
    }

    #[test]
    fn jitter_stays_inside_its_bounds_and_never_shortens_the_wait() {
        for raw in 0..2000u64 {
            let d = P.delay_for(2, None, raw);
            assert!(d >= Duration::from_millis(200), "{raw}: {d:?} below the base wait");
            assert!(d <= P.max_delay, "{raw}: {d:?} past the ceiling");
            assert_eq!(P.delay_for(1, Some(Duration::from_secs(3)), raw), Duration::from_secs(3), "{raw}");
        }
        // It actually varies, otherwise it is not a spread.
        let distinct: Vec<_> = (0..200).map(|raw| P.delay_for(3, None, raw)).collect();
        assert!(distinct.iter().collect::<std::collections::HashSet<_>>().len() > 10);
    }

    #[test]
    fn spread_is_deterministic_and_zero_when_there_is_no_room() {
        assert_eq!(spread(7, 0), 0);
        assert_eq!(spread(7, 10), spread(7, 10));
        for raw in 0..500u64 {
            assert!(spread(raw, 25) < 25, "{raw}");
        }
        assert_eq!(spread(u64::MAX, 9), spread(u64::MAX, 9));
    }

    #[test]
    fn policy_validation() {
        assert_eq!(Policy::polite().validate(), Ok(()));
        assert_eq!(Policy::no_retry().validate(), Ok(()));
        assert_eq!(Policy::default(), Policy::polite());
        for bad in [
            Policy { attempts: 0, ..P },
            Policy { attempts: 9, ..P },
            Policy { max_concurrent: 0, ..P },
            Policy { max_concurrent: 65, ..P },
            Policy { requests_per_second: 0, ..P },
            Policy { requests_per_second: 51, ..P },
            Policy { base_delay: Duration::ZERO, ..P },
            Policy { base_delay: Duration::from_secs(30), ..P }, // base above max
            Policy { max_delay: Duration::from_secs(3601), total_budget: Duration::from_secs(3601), ..P },
        ] {
            assert_eq!(bad.validate(), Err(crate::Error::InvalidRequest), "{bad:?}");
        }
        assert_eq!(P.validate(), Ok(()));
    }

    #[test]
    fn token_bucket_allows_a_burst_then_refills_at_the_rate() {
        // 5 per second: 5 tokens up front, one more every 200 ms.
        let mut b = TokenBucket::new(5);
        for _ in 0..5 {
            assert_eq!(b.take(1000), 0, "the starting burst is free");
        }
        let wait = b.take(1000);
        assert!((199..=201).contains(&wait), "{wait} ms to earn the sixth token");
        assert_eq!(b.take(1201), 0, "one token has appeared 200 ms later");
        assert!((199..=201).contains(&b.take(1201)), "and the next is another 200 ms away");
        // A clock that never moves buys nothing.
        let mut stuck = TokenBucket::new(2);
        for _ in 0..2 {
            assert_eq!(stuck.take(0), 0, "the burst");
        }
        assert!(stuck.take(0) > 0, "no refill without elapsed time");
    }

    #[test]
    fn asking_again_halfway_does_not_reset_the_wait() {
        let mut slow = TokenBucket::new(1);
        assert_eq!(slow.take(0), 0);
        assert_eq!(slow.take(0), 1001, "1 rps: the next token is a full second away");
        assert_eq!(slow.take(500), 501, "still waiting, but half as long");
        assert_eq!(slow.take(1001), 0);
    }

    #[test]
    fn bucket_never_banks_more_than_one_second_of_work() {
        let mut b = TokenBucket::new(2);
        for _ in 0..2 {
            assert_eq!(b.take(0), 0);
        }
        assert!(b.take(0) > 0, "empty after the burst");
        // Two hours later there are still only 2 tokens, not 14400.
        assert_eq!((0..2).filter(|_| b.take(7_200_000) == 0).count(), 2);
        assert!(b.take(7_200_000) > 0);
    }

    #[test]
    fn tokio_clock_only_moves_forward() {
        let c = TokioClock::default();
        let mut last = c.now_ms();
        for _ in 0..50 {
            let now = c.now_ms();
            assert!(now >= last, "{now} before {last}");
            last = now;
        }
    }
}
