//! Server-rendered Tempest UI prototype.
//!
//! `cargo run -- snapshot <theme> <out.html>` writes a static page, used as
//! the design reference and for screenshots.

mod render;

use proto_core::World;
use render::Ui;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("snapshot") {
        let theme = args.get(2).map(String::as_str).unwrap_or("green");
        let out = args.get(3).map(String::as_str).unwrap_or("snapshot.html");
        let mut world = World::new(42);
        for _ in 0..40 {
            world.tick(250);
        }
        let theme = render::THEMES.iter().find(|(k, _)| *k == theme).map(|(k, _)| *k).unwrap_or("green");
        let ui = Ui { theme, ..Ui::default() };
        let html = render::page(&world, &ui, maud::html! {}).into_string();
        let html = html.replace("/tempest.css", "../shared/tempest.css");
        std::fs::write(out, html).expect("write snapshot");
        return;
    }
    eprintln!("usage: server-app snapshot <theme> <out.html>");
}
