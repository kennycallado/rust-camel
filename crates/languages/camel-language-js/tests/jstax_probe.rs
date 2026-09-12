//! Mission jstax evidence probe — Boa 0.22 per-eval cost decomposition.
//!
//! Diagnostic only (#[ignore]): measures Context::default vs realm-per-eval vs
//! parse vs cached-evaluate to ground the js-engine-cache design decision.
//! Never run in CI; canonical bench era-2 stays sealed.

use boa_engine::{Context, Script, Source};

const SRC: &str = "let acc = 0; for (let i = 0; i < 10; i++) { acc += i; } acc";
const SRC_NO_LEX: &str = "var acc = 0; for (var i = 0; i < 10; i++) { acc += i; } acc";

fn bench<F: FnMut()>(name: &str, iters: u32, mut f: F) {
    // warmup
    for _ in 0..20 {
        f();
    }
    let t = std::time::Instant::now();
    for _ in 0..iters {
        f();
    }
    let per = t.elapsed().as_nanos() / u128::from(iters);
    println!("{name:<42} {per:>9} ns/op");
}

#[test]
#[ignore = "slow test: diagnostic probe (mission jstax cost decomposition); run explicitly with --release -- --ignored --nocapture"]
fn decomposition() {
    let iters = 300;

    bench("Context::default()", iters, || {
        let _ = Context::default();
    });

    // Option 2 (exact semantics): persistent Context + fresh realm per eval.
    // create_realm = Realm::create + set_default_global_bindings.
    {
        let mut ctx = Context::default();
        bench("ctx.create_realm() [fresh realm]", iters, || {
            let _ = ctx.create_realm();
        });
    }

    // Parse cost on a fresh realm each time (exact-semantics world re-parses).
    {
        let mut ctx = Context::default();
        bench("create_realm + Script::parse [no-lex src]", iters, || {
            let _r = ctx.create_realm();
            let _s = Script::parse(Source::from_bytes(SRC_NO_LEX.as_bytes()), None, &mut ctx);
        });
    }

    // D'-style: stable realm, parse same no-lex source repeatedly (first real
    // parse only; subsequent parses hit the same realm; measures parse floor).
    {
        let mut ctx = Context::default();
        bench("Script::parse stable-realm [no-lex]", iters, || {
            let _s = Script::parse(Source::from_bytes(SRC_NO_LEX.as_bytes()), None, &mut ctx);
        });
    }

    // Cached-script evaluate on stable realm (the reuse win, D' world).
    {
        let mut ctx = Context::default();
        let script = Script::parse(Source::from_bytes(SRC.as_bytes()), None, &mut ctx).unwrap();
        bench("cached Script::evaluate [lex src]", iters, || {
            let _ = script.evaluate(&mut ctx);
        });
    }

    // TODAY's full per-eval cost (fresh Context + parse + evaluate).
    bench("today: Context::default+parse+eval", iters, || {
        let mut ctx = Context::default();
        let s = Script::parse(Source::from_bytes(SRC.as_bytes()), None, &mut ctx).unwrap();
        let _ = s.evaluate(&mut ctx);
    });

    // Binding registration cost proxy (register_console + camel global build
    // are our code): measure global_object set of one property.
    {
        let mut ctx = Context::default();
        let g = ctx.global_object();
        bench("set one global prop", iters, || {
            let _ = g.set(
                boa_engine::js_string!("probe"),
                boa_engine::JsValue::from(42),
                false,
                &mut ctx,
            );
        });
    }

    // Option 2 full path: persistent ctx + ENTER fresh realm + parse + eval
    // (exact semantics; the only legal reuse under the seal).
    // NOTE: create_realm() returns to the OLD realm; we must enter_realm the
    // new one explicitly, else repeated `let` parses hit the old realm's
    // declarative env (the panic this note replaced demonstrated exactly that).
    {
        let mut ctx = Context::default();
        bench("opt2: fresh realm+parse+eval", iters, || {
            let new = ctx.create_realm().expect("create_realm");
            let _old = ctx.enter_realm(new);
            let s = Script::parse(Source::from_bytes(SRC.as_bytes()), None, &mut ctx)
                .expect("parse in fresh realm");
            let _ = s.evaluate(&mut ctx);
        });
    }

    // Global scrub cost proxy: own_property_keys length walk.
    {
        let mut ctx = Context::default();
        let keys = ctx.global_object().own_property_keys(&mut ctx).unwrap();
        println!("global own_property_keys count (baseline): {}", keys.len());
    }
}
