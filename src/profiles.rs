
use std::collections::HashMap;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

const API_BASE: &str = "https://api.opnsoc.org/player/";
const FETCH_TIMEOUT_SECS: u64 = 10;
const HEAD_SIZE: u32 = 100;

pub struct Fetched {
    pub username: Option<String>,
    pub head: Option<image::RgbaImage>,
}

enum State {
    Pending,
    Ready(Option<image::RgbaImage>),
    Failed,
}

#[derive(Default)]
pub struct Profiles {
    state: HashMap<String, State>,
    tex: HashMap<String, Option<TextureHandle>>,
    to_fetch: Vec<String>,
}

impl Profiles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&mut self, key: &str) {
        if self.state.contains_key(key) {
            return;
        }
        self.state.insert(key.to_string(), State::Pending);
        self.to_fetch.push(key.to_string());
    }

    pub fn drain_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.to_fetch)
    }

    pub fn set(&mut self, key: String, res: Result<Fetched, String>) {
        let state = match res {
            Ok(f) => State::Ready(f.head),
            Err(_) => State::Failed,
        };
        self.state.insert(key, state);
    }

    pub fn head(&mut self, ctx: &Context, key: &str) -> Option<TextureHandle> {
        if let Some(cached) = self.tex.get(key) {
            return cached.clone();
        }
        if !self.state.contains_key(key) {
            self.request(key);
            return None;
        }
        let tex = match self.state.get(key) {
            Some(State::Ready(Some(img))) => {
                let color = ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                Some(ctx.load_texture(format!("head::{key}"), color, TextureOptions::LINEAR))
            }
            _ => None,
        };
        if matches!(self.state.get(key), Some(State::Ready(_)) | Some(State::Failed)) {
            self.tex.insert(key.to_string(), tex.clone());
        }
        tex
    }
}

pub fn fetch(key: &str) -> Result<Fetched, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build();

    if key.starts_with("http") {
        let head = fetch_skin(&agent, key).and_then(|s| crate::render::head3d::render(&s, HEAD_SIZE));
        return match head {
            Some(h) => Ok(Fetched { username: None, head: Some(h) }),
            None => Err("skin fetch/render failed".into()),
        };
    }

    let body = agent
        .get(&format!("{API_BASE}{key}"))
        .call()
        .map_err(|e| format!("profile request: {e}"))?
        .into_string()
        .map_err(|e| format!("profile body: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("profile json: {e}"))?;
    let player = &json["data"]["player"];

    let username = player["username"].as_str().filter(|s| !s.is_empty()).map(String::from);
    let head = player["skin_texture"]
        .as_str()
        .filter(|s| !s.is_empty())
        .and_then(|url| fetch_skin(&agent, url))
        .and_then(|s| crate::render::head3d::render(&s, HEAD_SIZE));

    if username.is_none() && head.is_none() {
        return Err("player not found".into());
    }
    Ok(Fetched { username, head })
}

fn fetch_skin(agent: &ureq::Agent, url: &str) -> Option<image::RgbaImage> {
    let resp = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes).ok()?;
    Some(image::load_from_memory(&bytes).ok()?.to_rgba8())
}
