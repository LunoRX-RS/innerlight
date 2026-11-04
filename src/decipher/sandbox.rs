//! A sandboxed, pure-Rust JavaScript evaluator (`boa_engine`) for the extracted transforms.
//!
//! Isolation: the engine is created with no host objects at all (no `fetch`, `require`, `process`, `console`,
//! timers, file or network access; `boa_runtime` is not linked). The context lives on its own thread and is only
//! reachable through a channel. Limits: a loop-iteration budget and a recursion limit inside the engine, a wall-clock
//! timeout around every call (a stuck evaluation is abandoned and its thread ends when the loop budget runs out),
//! length caps on the output, and caps on the native string builders (`repeat`, `padStart`, `padEnd`, `Array#join`).
//! Boa has no memory ceiling of its own; the caps above are what bounds memory, and are best effort.

use super::DecipherError;
use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{Context, JsError, JsString, Source, js_string};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Limits of one [`Sandbox`].
#[derive(Debug, Clone, Copy)]
pub struct SandboxLimits {
    /// Wall-clock limit of one call (`eval` / `call`). Default 250 ms.
    pub call_timeout: Duration,
    /// Wall-clock limit for loading the program. Default 20 s.
    pub load_timeout: Duration,
    /// Loop iterations per call frame before the engine aborts. Default 3 000 000.
    pub loop_iterations: u64,
    /// Nested JS call depth. Default 256.
    pub recursion: usize,
    /// Longest accepted result, in UTF-16 units. Default 8192.
    pub max_output: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_millis(250),
            load_timeout: Duration::from_secs(20),
            loop_iterations: 3_000_000,
            recursion: 256,
            max_output: 8192,
        }
    }
}

/// Runs inside the engine before the program: caps the native string builders so one call cannot allocate without
/// bound. Strict mode, no globals leaked.
const HARDEN: &str = "'use strict';(function(){var cap=1048576,S=String.prototype,A=Array.prototype;\
var rep=S.repeat,ps=S.padStart,pe=S.padEnd,jn=A.join;\
function put(o,k,f){Object.defineProperty(o,k,{value:f,configurable:true,writable:true,enumerable:false})}\
put(S,'repeat',function repeat(n){if(!(n>=0)||n*this.length>cap)throw new RangeError('limit');return rep.call(this,n)});\
put(S,'padStart',function padStart(n,f){if(!(n<=cap))throw new RangeError('limit');return ps.call(this,n,f)});\
put(S,'padEnd',function padEnd(n,f){if(!(n<=cap))throw new RangeError('limit');return pe.call(this,n,f)});\
put(A,'join',function join(s){if(this.length>cap)throw new RangeError('limit');return jn.call(this,s)});\
})();";

struct Job {
    code: String,
    arg: Option<String>,
    strict_string: bool,
    reply: SyncSender<Result<String, DecipherError>>,
}

struct Worker {
    tx: Sender<Job>,
}

/// One JavaScript context holding a program, behind a worker thread. `Send + Sync`; calls are serialised.
/// After a timeout or a crashed worker the context is rebuilt from the program on the next call.
pub struct Sandbox {
    program: Arc<str>,
    limits: SandboxLimits,
    worker: Mutex<Option<Worker>>,
    /// Calls that ended in an error since the context was built; see [`CONTEXT_REFRESH_AFTER`].
    failures: AtomicU32,
}

/// The engine does not give back every stack slot of a call that ends in a thrown error. After enough of them even
/// a correct call reports "maximum stack size". Probing a script tries hundreds of wrong candidates in one sandbox,
/// so a context that has seen this many failed calls is rebuilt from the program before the next call.
const CONTEXT_REFRESH_AFTER: u32 = 8;

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sandbox(..)")
    }
}

fn classify(err: &JsError) -> DecipherError {
    match err.as_engine() {
        Some(EngineError::RuntimeLimit(
            RuntimeLimitError::LoopIteration | RuntimeLimitError::Recursion | RuntimeLimitError::StackSize,
        )) => DecipherError::SandboxBudget,
        _ => DecipherError::SandboxFailed,
    }
}

fn debug(msg: &str) {
    if std::env::var_os("INNERLIGHT_DECIPHER_DEBUG").is_some() {
        eprintln!("[decipher] {msg}");
    }
}

fn run_job(ctx: &mut Context, job: &Job, max_output: usize) -> Result<String, DecipherError> {
    if let Some(arg) = &job.arg {
        ctx.global_object()
            .set(js_string!("__a"), JsString::from(arg.as_str()), true, ctx)
            .map_err(|_| DecipherError::SandboxFailed)?;
    }
    let value = ctx.eval(Source::from_bytes(job.code.as_bytes())).map_err(|e| {
        debug(&format!("eval error: {e}"));
        classify(&e)
    })?;
    let js = match (value.as_string(), job.strict_string) {
        (Some(s), _) => s,
        (None, true) => return Err(DecipherError::BadOutput),
        (None, false) => value.to_string(ctx).map_err(|e| classify(&e))?,
    };
    if js.len() > max_output {
        return Err(DecipherError::BadOutput);
    }
    js.to_std_string().map_err(|_| DecipherError::BadOutput)
}

fn worker_main(
    program: Arc<str>,
    limits: SandboxLimits,
    init: SyncSender<Result<(), DecipherError>>,
    rx: mpsc::Receiver<Job>,
) {
    let mut ctx = match Context::builder().build() {
        Ok(c) => c,
        Err(_) => {
            let _ = init.send(Err(DecipherError::SandboxFailed));
            return;
        }
    };
    {
        let l = ctx.runtime_limits_mut();
        l.set_loop_iteration_limit(limits.loop_iterations);
        l.set_recursion_limit(limits.recursion);
    }
    let loaded = ctx
        .eval(Source::from_bytes(HARDEN.as_bytes()))
        .and_then(|_| ctx.eval(Source::from_bytes(program.as_bytes())))
        .map(|_| ())
        .map_err(|e| {
            debug(&format!("program load error: {e}"));
            classify(&e)
        });
    let ok = loaded.is_ok();
    let _ = init.send(loaded);
    if !ok {
        return;
    }
    while let Ok(job) = rx.recv() {
        let result = run_job(&mut ctx, &job, limits.max_output);
        let _ = job.reply.send(result);
    }
}

impl Sandbox {
    /// Evaluates `program` (strict mode is the program's own business) in a fresh context. The program must only
    /// define things; `Err` when it throws, exceeds a budget or does not finish within the load timeout.
    pub fn new(program: &str, limits: SandboxLimits) -> Result<Sandbox, DecipherError> {
        let sb = Sandbox { program: Arc::from(program), limits, worker: Mutex::new(None), failures: AtomicU32::new(0) };
        let w = sb.spawn()?;
        *sb.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
        Ok(sb)
    }

    fn spawn(&self) -> Result<Worker, DecipherError> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let program = Arc::clone(&self.program);
        let limits = self.limits;
        std::thread::Builder::new()
            .name("innerlight-sandbox".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || worker_main(program, limits, init_tx, rx))
            .map_err(|_| DecipherError::SandboxFailed)?;
        match init_rx.recv_timeout(self.limits.load_timeout) {
            Ok(Ok(())) => Ok(Worker { tx }),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err(DecipherError::SandboxTimeout),
            Err(RecvTimeoutError::Disconnected) => Err(DecipherError::SandboxFailed),
        }
    }

    fn run(&self, code: String, arg: Option<String>, strict_string: bool) -> Result<String, DecipherError> {
        let mut guard = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if self.failures.load(Ordering::Relaxed) >= CONTEXT_REFRESH_AFTER {
            *guard = None;
        }
        if guard.is_none() {
            self.failures.store(0, Ordering::Relaxed);
            *guard = Some(self.spawn()?);
        }
        let Some(worker) = guard.as_ref() else { return Err(DecipherError::SandboxFailed) };
        let (reply, answer) = mpsc::sync_channel(1);
        if worker.tx.send(Job { code, arg, strict_string, reply }).is_err() {
            *guard = None;
            return Err(DecipherError::SandboxFailed);
        }
        match answer.recv_timeout(self.limits.call_timeout) {
            Ok(r) => {
                if r.is_err() {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                }
                r
            }
            Err(RecvTimeoutError::Timeout) => {
                // Abandon the context: the stuck thread ends when the engine's own loop budget runs out.
                *guard = None;
                Err(DecipherError::SandboxTimeout)
            }
            Err(RecvTimeoutError::Disconnected) => {
                *guard = None;
                Err(DecipherError::SandboxFailed)
            }
        }
    }

    /// Evaluates arbitrary code inside the sandbox and returns the completion value as a string (numbers and
    /// booleans are converted). Mainly for tests of the sandbox itself.
    pub fn eval(&self, code: &str) -> Result<String, DecipherError> {
        self.run(code.to_owned(), None, false)
    }

    /// Calls a prepared call expression such as `Fn(__a)` with `arg` bound to the global `__a`. The result must be
    /// a string no longer than [`SandboxLimits::max_output`].
    pub fn call(&self, template: &str, arg: &str) -> Result<String, DecipherError> {
        if !template.contains("__a") || template.len() > 256 {
            return Err(DecipherError::BadInput);
        }
        self.run(template.to_owned(), Some(arg.to_owned()), true)
    }
}
