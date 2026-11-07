use boost_core::Engine;
fn main() {
    let gain: f32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1.0);
    println!("outputs: {:?}", Engine::outputs().iter().map(|d| d.id.clone()).collect::<Vec<_>>());
    let mut e = Engine::start(None, gain).expect("start failed");
    println!("started, output={}", e.output_id());
    for i in 0..8 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let m = e.meter();
        println!("t={} running={} peak={:.3} red={:.1}dB", i, e.is_running(), m.peak, m.reduction_db);
    }
    e.stop();
    println!("stopped");
}
