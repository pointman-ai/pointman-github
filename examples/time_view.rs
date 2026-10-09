//! How long `view` takes for each link given: `cargo run --release --example time_view -- <url>…`.
fn main() {
    let gh = github::find_gh("", None).expect("gh");
    for url in std::env::args().skip(1) {
        let link = github::parse(&url).unwrap();
        let t = std::time::Instant::now();
        let v = github::GitHub::new(&gh).view(&link);
        println!("{:>6} ms  {url}  {}", t.elapsed().as_millis(), if v.is_ok() { "ok" } else { "error" });
    }
}
