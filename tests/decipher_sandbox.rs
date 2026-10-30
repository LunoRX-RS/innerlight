//! The JS sandbox: no host functions, bounded work, bounded output, argument passed as data.

mod decipher_common;
use decipher_common::*;
use innerlight::decipher::{DecipherError, Sandbox, SandboxLimits, ScriptDecipher};
use std::time::{Duration, Instant};

fn sb() -> Sandbox {
    Sandbox::new("", SandboxLimits::default()).unwrap()
}

fn is_limit(e: &DecipherError) -> bool {
    matches!(e, DecipherError::SandboxTimeout | DecipherError::SandboxBudget)
}

#[test]
fn types_are_send_and_sync() {
    fn assert_ss<T: Send + Sync>() {}
    assert_ss::<Sandbox>();
    assert_ss::<ScriptDecipher>();
    assert_ss::<innerlight::decipher::ScriptCache>();
}

#[test]
fn many_failed_calls_do_not_make_a_correct_call_fail() {
    // Probing a script tries hundreds of wrong candidates in one sandbox; each ends in a thrown error from a nested
    // call. The next correct call must still work.
    let s = Sandbox::new(
        "function deep(n){if(n==0)throw new TypeError('x');return deep(n-1)+1}function ok(a){return a+'!'}",
        SandboxLimits::default(),
    )
    .unwrap();
    for _ in 0..400 {
        assert!(s.call("deep(40)+__a", "x").is_err());
    }
    assert_eq!(s.call("ok(__a)", "fine").unwrap(), "fine!");
}

#[test]
fn infinite_loops_hit_the_limits_and_the_sandbox_recovers() {
    let s = sb();
    for code in ["while(true){}", "for(;;){}", "do{}while(1)", "var i=0;while(i>=0){i++}"] {
        let t = Instant::now();
        let e = s.eval(code).unwrap_err();
        assert!(is_limit(&e), "{code}: {e:?}");
        assert!(t.elapsed() < Duration::from_secs(5), "{code} took {:?}", t.elapsed());
    }
    assert_eq!(s.eval("1+1").unwrap(), "2", "a fresh context after a timeout");
}

#[test]
fn loop_budget_is_deterministic() {
    let limits =
        SandboxLimits { loop_iterations: 1000, call_timeout: Duration::from_secs(20), ..SandboxLimits::default() };
    let s = Sandbox::new("", limits).unwrap();
    assert_eq!(s.eval("for(;;){}"), Err(DecipherError::SandboxBudget));
    assert_eq!(s.eval("var n=0;for(var i=0;i<500;i++)n+=i;n").unwrap(), "124750");
    assert_eq!(s.eval("function f(){return f()}f()"), Err(DecipherError::SandboxBudget));
}

#[test]
fn no_host_functions() {
    let s = sb();
    for name in [
        "fetch",
        "require",
        "process",
        "console",
        "setTimeout",
        "setInterval",
        "XMLHttpRequest",
        "WebSocket",
        "Deno",
        "Bun",
        "window",
        "document",
        "navigator",
        "module",
        "Buffer",
        "readFile",
    ] {
        assert_eq!(s.eval(&format!("typeof {name}")).unwrap(), "undefined", "{name}");
    }
    for call in [
        "fetch('https://example.com')",
        "require('fs')",
        "process.exit(1)",
        "console.log(1)",
        "setTimeout(function(){},1)",
        "new XMLHttpRequest()",
        "Function('return process')()",
        "(function(){return this.process.env})()",
        "globalThis.require('child_process')",
    ] {
        assert_eq!(s.eval(call), Err(DecipherError::SandboxFailed), "{call}");
    }
    // the global object carries only the language's own builtins
    assert_eq!(s.eval("Object.getOwnPropertyNames(globalThis).filter(function(k){return /fetch|require|process|console|timeout|socket|http|fs/i.test(k)}).join()").unwrap(), "");
}

#[test]
fn huge_output_and_allocations_are_rejected() {
    let s = sb();
    assert_eq!(s.eval("'a'.repeat(100000)"), Err(DecipherError::BadOutput));
    assert_eq!(s.eval("'a'.repeat(8192).length").unwrap(), "8192");
    assert_eq!(s.eval("'a'.repeat(Math.pow(2,31))"), Err(DecipherError::SandboxFailed));
    assert_eq!(s.eval("'ab'.repeat(600000)"), Err(DecipherError::SandboxFailed));
    assert_eq!(s.eval("'a'.padEnd(1e9)"), Err(DecipherError::SandboxFailed));
    assert_eq!(s.eval("'a'.padStart(1e9,'xy')"), Err(DecipherError::SandboxFailed));
    assert_eq!(s.eval("new Array(5e6).join('x')"), Err(DecipherError::SandboxFailed));
    let limits = SandboxLimits { max_output: 16, ..SandboxLimits::default() };
    let small = Sandbox::new("var f=function(a){return a+a}", limits).unwrap();
    assert_eq!(small.call("f(__a)", "12345678").unwrap(), "1234567812345678");
    assert_eq!(small.call("f(__a)", "123456789"), Err(DecipherError::BadOutput));
}

#[test]
fn call_requires_a_string_result_and_a_placeholder() {
    let s = Sandbox::new("var n=function(a){return a.length};var u=function(){};", SandboxLimits::default()).unwrap();
    assert_eq!(s.call("n(__a)", "abc"), Err(DecipherError::BadOutput));
    assert_eq!(s.call("u(__a)", "abc"), Err(DecipherError::BadOutput));
    assert_eq!(s.call("n(1)", "abc"), Err(DecipherError::BadInput));
    assert_eq!(s.call(&format!("n(__a){}", " ".repeat(300)), "abc"), Err(DecipherError::BadInput));
}

#[test]
fn argument_is_data_not_code() {
    let s = Sandbox::new(
        "var f=function(a){return a};var r=function(a){return a.split('').reverse().join('')}",
        SandboxLimits::default(),
    )
    .unwrap();
    for arg in
        ["abc", "a\"b\\c\n'd`${x}", "');while(true){}//", "\u{e9}\u{4e2d}\u{1f600}x", "\\u0041", "</script>", "\0\u{1}"]
    {
        assert_eq!(s.call("f(__a)", arg).as_deref(), Ok(arg), "{arg:?}");
    }
    for arg in ["abc", "a\"b\\c\n'd`${x}", "');while(true){}//", "\u{e9}\u{4e2d}x"] {
        assert_eq!(s.call("r(__a)", arg).unwrap(), arg.chars().rev().collect::<String>(), "{arg:?}");
    }
}

#[test]
fn program_load_failures_are_typed() {
    assert_eq!(
        Sandbox::new("throw new Error('x')", SandboxLimits::default()).err(),
        Some(DecipherError::SandboxFailed)
    );
    assert_eq!(Sandbox::new("this is not js", SandboxLimits::default()).err(), Some(DecipherError::SandboxFailed));
    let tight = SandboxLimits { loop_iterations: 100, ..SandboxLimits::default() };
    assert_eq!(Sandbox::new("for(;;){}", tight).err(), Some(DecipherError::SandboxBudget));
}

#[test]
fn concurrent_calls_are_serialised_and_correct() {
    let d = std::sync::Arc::new(decipher(SCRIPT_A));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let d = d.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    assert_eq!(d.signature(SIG_IN).unwrap(), ref_sig_a(SIG_IN));
                    let n = format!("Throttle{i}x");
                    assert_eq!(d.throttle_value(&n).unwrap(), ref_n_classic(&n));
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn a_transform_that_loops_forever_is_an_error_not_a_hang() {
    // a script whose n function is fine during analysis but loops for one particular input
    let js =
        SCRIPT_A.replace("var b=a.split(\"\"),c=[3,7,11];", "var b=a.split(\"\"),c=[3,7,11];if(a==='HANG')for(;;){}");
    let d = decipher(&js);
    let t = Instant::now();
    assert!(is_limit(&d.throttle_value("HANG").unwrap_err()));
    assert!(t.elapsed() < Duration::from_secs(5));
    // and the decipher keeps working afterwards
    assert_eq!(d.throttle_value("Qa3_9-XyZ0bk").unwrap(), ref_n_classic("Qa3_9-XyZ0bk"));
}
