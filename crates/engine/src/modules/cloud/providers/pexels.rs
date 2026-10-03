//! Pexels: stock photos and videos. `Authorization: <key>`, no scheme.
//!
//! Pexels' rules for apps: show a prominent link to Pexels, credit the
//! photographer where possible ("Photo by X on Pexels"). Credit is not
//! required by the licence, so a Pexels item alone never makes the export
//! write a credits file, but it is listed when one is written.

use serde_json::Value;

use super::super::http;
use super::super::provenance::Licence;
use super::super::registry::Account;
use super::super::stock::{StockHit, StockKind, StockPage, StockQuery, StockSearch};
use super::super::TestReport;

pub struct Pexels {
    account: Account,
    key: String,
}

fn licence() -> Licence {
    Licence::free(
        "LicenseRef-Pexels",
        "Pexels License",
        "https://www.pexels.com/license/",
    )
}

impl Pexels {
    pub fn new(account: Account, key: String) -> Self {
        Self { account, key }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let response = http::agent()
            .get(&self.account.url(path))
            .set("Authorization", &self.key)
            .call()
            .map_err(|e| http::describe(e, &self.key))?;
        http::read_json(response)
    }

    pub fn test(&self) -> TestReport {
        match self.get("v1/search?query=nature&per_page=1") {
            Ok(_) => TestReport::ok("Connected"),
            Err(message) => TestReport::failed(message),
        }
    }
}

impl StockSearch for Pexels {
    fn kinds(&self) -> &'static [StockKind] {
        &[StockKind::Video, StockKind::Photo]
    }

    fn search(&self, query: &StockQuery) -> Result<StockPage, String> {
        let per_page = query.per_page.clamp(1, 80);
        let params = format!(
            "query={}&per_page={per_page}&page={}",
            http::encode(query.text.trim()),
            query.page.max(1)
        );
        let (body, hits) = match query.kind {
            StockKind::Photo => {
                let body = self.get(&format!("v1/search?{params}"))?;
                let hits = body
                    .get("photos")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(photo).collect())
                    .unwrap_or_default();
                (body, hits)
            }
            StockKind::Video => {
                let body = self.get(&format!("videos/search?{params}"))?;
                let hits = body
                    .get("videos")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(video).collect())
                    .unwrap_or_default();
                (body, hits)
            }
            StockKind::Sound => return Err("Pexels has no sounds".to_string()),
        };
        Ok(StockPage {
            hits,
            total: body
                .get("total_results")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            has_more: body.get("next_page").is_some_and(|n| !n.is_null()),
            attribution: "Photos and videos provided by Pexels".into(),
            attribution_url: "https://www.pexels.com".into(),
        })
    }
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn id(v: &Value) -> Option<String> {
    v.get("id").map(|i| match i {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn photo(v: &Value) -> Option<StockHit> {
    let id = id(v)?;
    let src = v.get("src")?;
    let creator = text(v, "photographer");
    let alt = text(v, "alt");
    Some(StockHit {
        provider: "pexels".into(),
        title: if alt.is_empty() {
            format!("Pexels photo {id}")
        } else {
            alt
        },
        credit: format!("Photo by {creator} on Pexels"),
        creator,
        creator_url: text(v, "photographer_url"),
        source_url: text(v, "url"),
        thumb_url: src
            .get("medium")
            .and_then(Value::as_str)
            .map(str::to_string),
        preview_url: None,
        // Twice the "large" size: about 1880 px across, plenty for a 1080p
        // canvas, a fraction of the original's size.
        download_url: src
            .get("large2x")
            .or_else(|| src.get("original"))
            .and_then(Value::as_str)?
            .to_string(),
        width: v.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
        height: v.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
        duration_seconds: 0.0,
        licence: licence(),
        kind: StockKind::Photo,
        id,
    })
}

fn video(v: &Value) -> Option<StockHit> {
    let id = id(v)?;
    let files: Vec<&Value> = v
        .get("video_files")
        .and_then(Value::as_array)?
        .iter()
        .filter(|f| text(f, "file_type") == "video/mp4" && f.get("link").is_some())
        .collect();
    let edge = |f: &Value| {
        let w = f.get("width").and_then(Value::as_u64).unwrap_or(0);
        let h = f.get("height").and_then(Value::as_u64).unwrap_or(0);
        w.max(h)
    };
    // The largest file up to 1920 on the long edge: full HD is what the
    // canvas is, and Pexels' 4K files are five times the download.
    let best = files
        .iter()
        .filter(|f| edge(f) <= 1920)
        .max_by_key(|f| edge(f))
        .or_else(|| files.iter().min_by_key(|f| edge(f)))?;
    let smallest = files.iter().filter(|f| edge(f) > 0).min_by_key(|f| edge(f));
    let user = v.get("user");
    let creator = user.map(|u| text(u, "name")).unwrap_or_default();
    Some(StockHit {
        provider: "pexels".into(),
        title: format!("Pexels video {id}"),
        credit: format!("Video by {creator} on Pexels"),
        creator,
        creator_url: user.map(|u| text(u, "url")).unwrap_or_default(),
        source_url: text(v, "url"),
        thumb_url: v.get("image").and_then(Value::as_str).map(str::to_string),
        preview_url: smallest.map(|f| text(f, "link")),
        download_url: text(best, "link"),
        width: best.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
        height: best.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
        duration_seconds: v.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
        licence: licence(),
        kind: StockKind::Video,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::http::test_server;
    use super::super::super::registry::ProviderKind;
    use super::*;

    fn pexels(url: &str) -> Pexels {
        let mut account = Account::of_kind(ProviderKind::Pexels);
        account.base_url = url.to_string();
        Pexels::new(account, "px-key".into())
    }

    #[test]
    fn videos_pick_the_full_hd_file_and_a_small_preview() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"total_results":2,"next_page":"https://x?page=2","videos":[{"id":42,"url":"https://www.pexels.com/video/42/","image":"https://i/42.jpg","duration":9,"user":{"name":"Jane","url":"https://www.pexels.com/@jane"},"video_files":[{"quality":"uhd","file_type":"video/mp4","width":3840,"height":2160,"link":"https://v/uhd.mp4"},{"quality":"hd","file_type":"video/mp4","width":1920,"height":1080,"link":"https://v/hd.mp4"},{"quality":"sd","file_type":"video/mp4","width":640,"height":360,"link":"https://v/sd.mp4"}]}]}"#.to_vec(),
        )]);
        let page = pexels(&server.url)
            .search(&StockQuery::new("sea waves", StockKind::Video))
            .unwrap();
        assert!(page.has_more);
        assert_eq!(page.attribution, "Photos and videos provided by Pexels");
        let hit = &page.hits[0];
        assert_eq!(hit.id, "42");
        assert_eq!(hit.download_url, "https://v/hd.mp4");
        assert_eq!(hit.preview_url.as_deref(), Some("https://v/sd.mp4"));
        assert_eq!(hit.credit, "Video by Jane on Pexels");
        assert!(!hit.licence.attribution_required);
        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(
            sent.request_line,
            "GET /videos/search?query=sea%20waves&per_page=24&page=1 HTTP/1.1"
        );
        assert_eq!(sent.header("authorization"), Some("px-key"));
    }

    #[test]
    fn photos_carry_the_photographer() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"total_results":1,"photos":[{"id":7,"width":4000,"height":3000,"url":"https://www.pexels.com/photo/7/","photographer":"Ann","photographer_url":"https://www.pexels.com/@ann","alt":"A red boat","src":{"original":"https://p/o.jpg","large2x":"https://p/l2.jpg","medium":"https://p/m.jpg"}}]}"#.to_vec(),
        )]);
        let page = pexels(&server.url)
            .search(&StockQuery::new("boat", StockKind::Photo))
            .unwrap();
        let hit = &page.hits[0];
        assert_eq!(hit.title, "A red boat");
        assert_eq!(hit.download_url, "https://p/l2.jpg");
        assert_eq!(hit.thumb_url.as_deref(), Some("https://p/m.jpg"));
        assert!(!page.has_more);
        assert!(server.requests.lock().unwrap()[0]
            .request_line
            .starts_with("GET /v1/search?query=boat"));
    }
}
