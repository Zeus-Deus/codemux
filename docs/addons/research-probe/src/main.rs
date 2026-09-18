use rquickjs::{Context, Function, Runtime};
use std::{env, time::{Duration, Instant}};
fn main() {
    let mode = env::args().nth(1).unwrap_or("ui".to_string());
    let rt = Runtime::new().unwrap();
    rt.set_memory_limit(64 * 1024 * 1024);
    rt.set_max_stack_size(512 * 1024);
    let deadline = Instant::now() + Duration::from_millis(250);
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let ctx = Context::full(&rt).unwrap();
    ctx.with(|ctx| {
        ctx.globals().set("__sendJson", Function::new(ctx.clone(), |message: String| { println!("{}", message); }).unwrap()).unwrap();
        let source = match mode.as_str() {
            "spin" => "while(true) {}".to_string(),
            "memory" => "globalThis.tooLarge = new ArrayBuffer(128*1024*1024);".to_string(),
            _ => std::fs::read_to_string("probe-ui.bundle.js").unwrap(),
        };
        if let Err(err) = ctx.eval::<(),_>(source) {
            eprintln!("ENGINE_ERROR {err}: {:?}", ctx.catch());
            if mode == "ui" { std::process::exit(1); }
        } else if mode != "ui" { panic!("limit failed"); }
    });
    if mode == "ui" {
        while rt.execute_pending_job().unwrap() {}
        ctx.with(|ctx| ctx.eval::<(),_>("__dispatchProbeEvent()").unwrap());
        while rt.execute_pending_job().unwrap() {}
    }
    println!("PROBE_OK {}",mode);
}
