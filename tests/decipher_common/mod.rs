//! Hand-authored mini player scripts in the structural style of the real one, with reference implementations of
//! what they compute. Invented names and constants; they exist only to prove the heuristics and the sandbox.
#![allow(dead_code)] // each test crate uses a different subset

use innerlight::decipher::{PlayerScript, ScriptDecipher};

pub fn script(js: &str) -> PlayerScript {
    PlayerScript::from_source("test1234".into(), js.into()).expect("valid script")
}

pub fn decipher(js: &str) -> ScriptDecipher {
    ScriptDecipher::new(&script(js)).expect("transforms located")
}

pub const SIG_IN: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_=";

fn swap(v: &mut [char], b: usize) {
    let i = b % v.len();
    v.swap(0, i);
}

/// Variant A: `reverse(63) splice(2) swap(12) reverse(42) splice(1)`.
pub fn ref_sig_a(s: &str) -> String {
    let mut v: Vec<char> = s.chars().collect();
    v.reverse();
    v.drain(..2);
    swap(&mut v, 12);
    v.reverse();
    v.drain(..1);
    v.into_iter().collect()
}

/// Variant B: `swap(7) reverse splice(3) swap(40)`.
pub fn ref_sig_b(s: &str) -> String {
    let mut v: Vec<char> = s.chars().collect();
    swap(&mut v, 7);
    v.reverse();
    v.drain(..3);
    swap(&mut v, 40);
    v.into_iter().collect()
}

/// The classic n transform of variant A.
pub fn ref_n_classic(s: &str) -> String {
    let c = [3u32, 7, 11];
    let mut b: Vec<char> =
        s.chars().enumerate().map(|(d, ch)| char::from_u32(97 + (ch as u32 + c[d % 3]) % 26).unwrap()).collect();
    b.reverse();
    b.into_iter().collect()
}

/// `Ef` of the marker-less variant: `(code*5 + i) % 26 + 65`.
pub fn ref_n_plain(s: &str) -> String {
    s.chars().enumerate().map(|(i, ch)| char::from_u32(65 + (ch as u32 * 5 + i as u32) % 26).unwrap()).collect()
}

pub const SCRIPT_A: &str = r#"var _yt_player={};(function(g){var window=this;'use strict';
var Zq={xK:function(a,b){a.splice(0,b)},Pm:function(a){a.reverse()},Ld:function(a,b){var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}};
var Wf=function(a){a=a.split("");Zq.Pm(a,63);Zq.xK(a,2);Zq.Ld(a,12);Zq.Pm(a,42);Zq.xK(a,1);return a.join("")};
var Nf=function(a){var b=a.split(""),c=[3,7,11];try{for(var d=0;d<b.length;d++){var e=b[d].charCodeAt(0);b[d]=String.fromCharCode(97+(e+c[d%3])%26)}b.reverse();if(b.length<2)throw 1}catch(f){return"enhanced_except_Qw1Zr_w8_"+a}return b.join("")};
var cfg={signatureTimestamp:19900,other:1};
var unrelated=function(a,b){return a+b};
})(_yt_player);"#;

/// Different names, string keys, splice-style swap, a function declaration as entry, `n` found by its call site.
pub const SCRIPT_B: &str = r#"(function(){
var Ab={"k1":function(a){a.reverse()},"k2":function(a,b){a.splice(0,b)},"k3":function(a,b){a.splice(0,1,a.splice(b%a.length,1,a[0])[0])}};
function Cd(x){x=x.split("");Ab.k3(x,7);Ab.k1(x,0);Ab.k2(x,3);Ab.k3(x,40);return x.join("")}
var Ef=function(a){var b=a.split(""),c=5;for(var i=0;i<b.length;i++){b[i]=String.fromCharCode(65+(b[i].charCodeAt(0)*c+i)%26)}return b.join("")};
var Hh=function(u){var n=u.get("n");if(n){n=Ef(n);u.set("n",n)}return u};
var Pp=function(a){return Cd(decodeURIComponent(a))};
var cfg={"signatureTimestamp":"20123"};
})();"#;

/// The `n` function sits in an array and is called as `Ar[0](n)` after `get("n")`; the signature entry is only
/// recognisable from its `decodeURIComponent` call site because the helper has a single kind of member... plus one.
pub const SCRIPT_C: &str = r#"var Hq={zz:function(a){a.reverse()},yy:function(a,b){a.splice(0,b)}};
var Sig=function(a){a=a.split("");Hq.zz(a);Hq.yy(a,5);Hq.zz(a);return a.join("")};
var Ef=function(a){var b=a.split(""),c=5;for(var i=0;i<b.length;i++){b[i]=String.fromCharCode(65+(b[i].charCodeAt(0)*c+i)%26)}return b.join("")};
var Ar=[Ef];
var Go=function(u){var n=u.get(String.fromCharCode(110));if(n){n=Ar[0](n);u.set("n",n)}};
var Use=function(c){return Sig(decodeURIComponent(c))};
sts:20999;"#;

/// Multiplexed: one dispatcher `D(a,b,c)` with `e=b^a`, a string table, helper members reached by name, the failure
/// marker living in the table. `D(16,1453,s)` is the signature, `D(1,1300,n)` the `n` transform.
pub const SCRIPT_M: &str = r#"var _yt_player={};(function(g){var window=this;'use strict';
var T="local;splice;reverse;length;1;1ZzQ9xYw_w8_;n".split(";");
var H={Q1:function(a,b){a[T[1]](0,b)},R2:function(a){a[T[2]]()},S3:function(a,b){var c=a[0];a[0]=a[b%a[T[3]]];a[b%a[T[3]]]=c}};
var D=function(a,b,c){var e=b^a,h;
if(a==16){var f=c.split("");H.R2(f,1);H.S3(f,e%7);H.Q1(f,3);H.S3(f,4);h=f.join("")}
if(a==1){try{var k=c.split("");for(var i=0;i<k.length;i++)k[i]=String.fromCharCode(97+(k[i].charCodeAt(0)+e+i)%26);h=k.join("")}catch(x){h=T[e^1296]+c}}
if(a==72){h=c.length}
return h};
g.X=function(s){return D(16,1453,s)};
g.Y=function(u,n){u.set("n",D(1,1300,n))};
g.Z=function(u){return D(72,1500,u)};
var k={signatureTimestamp:20500};
})(_yt_player);"#;

/// Multiplexed, the 2026-10-30 shape: the signature is `D(16,1453,s)` as in `SCRIPT_M`, but the `n` dispatcher `W` runs
/// several guarded branches one after the other, so a wrong selector throws (`d` is undefined) and the `n` branch is
/// reached only with a selector from 27 to 31. No call site names it: `g.Y` computes both arguments. The failure
/// marker is read in a `catch` as `T[e^1239]+c`.
pub const SCRIPT_N: &str = r#"var _yt_player={};(function(g){var window=this;'use strict';
var T="local;splice;reverse;length;1;7Qm2ZkYw1x_w8_;n".split(";");
var H={Q1:function(a,b){a[T[1]](0,b)},R2:function(a){a[T[2]]()},S3:function(a,b){var c=a[0];a[0]=a[b%a[T[3]]];a[b%a[T[3]]]=c}};
var D=function(a,b,c){var e=b^a,h;
if(a==16){var f=c.split("");H.R2(f,1);H.S3(f,e%7);H.Q1(f,3);H.S3(f,4);h=f.join("")}
return h};
var W=function(a,b,c,d){var e=b^a,m;
if((a^50)<23&&(a>>2&15)>=5){m=d[T[3]]}
(a+5^30)<a&&(a-2|42)>=a&&(c=c%d[T[3]]);
if((a^26)<14&&a+2>>3>=2){try{var k=c.split("");for(var i=0;i<k.length;i++)k[i]=String.fromCharCode(97+(k[i].charCodeAt(0)+e+i)%26);m=k.join("")}catch(x){m=T[e^1239]+c}}
(a&110)==a&&d[T[3]]!=0&&(c=0);
return m};
g.X=function(s){return D(16,1453,s)};
g.Y=function(u,n){var q=7;u.set("n",W(q^20,q^1217,n))};
var k={signatureTimestamp:20600};
})(_yt_player);"#;

pub fn ref_n_n(s: &str) -> String {
    s.chars().enumerate().map(|(i, ch)| char::from_u32(97 + (ch as u32 + 1234 + i as u32) % 26).unwrap()).collect()
}

/// Same dispatcher but the string table has no failure marker: only the brute-force stage can find `n`.
pub fn script_m_without_marker() -> String {
    SCRIPT_M.replace("1ZzQ9xYw_w8_", "plain")
}

pub fn ref_sig_m(s: &str) -> String {
    let e = 1453usize ^ 16;
    let mut v: Vec<char> = s.chars().collect();
    v.reverse();
    swap(&mut v, e % 7);
    v.drain(..3);
    swap(&mut v, 4);
    v.into_iter().collect()
}

pub fn ref_n_m(s: &str, b: u32) -> String {
    let e = b ^ 1;
    s.chars().enumerate().map(|(i, ch)| char::from_u32(97 + (ch as u32 + e + i as u32) % 26).unwrap()).collect()
}
