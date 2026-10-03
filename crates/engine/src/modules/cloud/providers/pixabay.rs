//! Pixabay: stock photos and videos. The key is a `key=` query parameter,
//! which is why every error from here is passed through `http::describe`
//! with the key: a transport error prints the URL.
//!
//! Pixabay's rules: cache answers for 24 hours (`stock::cached_search` does),
//! download rather than hotlink, and show where the results come from. The
//! Pixabay Content License needs no credit.

use serde_json::Value;

use super::super::http;
use super::super::provenance::Licence;
use super::super::registry::Account;
use super::super::stock::{StockHit, StockKind, StockPage, StockQuery, StockSearch};
use super::super::TestReport;

pub struct Pixabay {
    account: Account,
    key: String,
}

fn licence() -> Licence {
    Licence {
        note: "Music and some content can still raise Content ID claims".into(),
        ..Licence::free(
            "LicenseRef-Pixabay",
            "Pixabay Content License",
            "https://pixabay.com/service/license-summary/",
        )
    }
}

impl Pixabay {
    pub fn new(account: Account, key: String) -> Self {
        Self { account, key }
    }

    fn get(&self, path: &str, params: &str) -> Result<Value, String> {
        let url = format!(
            "{}?key={}&{params}",
            self.account.url(path),
            http::encode(&self.key)
        );
        let response = http::agent()
            .get(&url)
            .call()
            .map_err(|e| http::describe(e, &self.key))?;
        http::read_json(response)
    }

    pub fn test(&self) -> TestReport {
        match self.get("api/", "q=nature&per_page=3") {
            Ok(_) => TestReport::ok("Connected"),
            Err(message) => TestReport::failed(message),
        }
    }
}

impl StockSearch for Pixabay {
    fn kinds(&self) -> &'static [StockKind] {
        &[StockKind::Video, StockKind::Photo]
    }

    fn search(&self, query: &StockQuery) -> Result<StockPage, String> {
        // Pixabay refuses fewer than 3 per page.
        let params = format!(
            "q={}&per_page={}&page={}&safesearch=true",
            http::encode(query.text.trim()),
            query.per_page.clamp(3, 200),
            query.page.max(1)
        );
        let (path, extra) = match query.kind {
            StockKind::Photo => ("api/", "&image_type=photo"),
            StockKind::Video => ("api/videos/", ""),
            StockKind::Sound => return Err("Pixabay's API has no sounds".to_string()),
        };
        let body = self.get(path, &format!("{params}{extra}"))?;
        let hits: Vec<StockHit> = body
            .get("hits")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(|h| hit(h, query.kind)).collect())
            .unwrap_or_default();
        let total = body.get("totalHits").and_then(Value::as_u64).unwrap_or(0);
        let seen = u64::from(query.page.max(1)) * u64::from(query.per_page.clamp(3, 200));
        Ok(StockPage {
            has_more: seen < total,
            hits,
            total,
            attribution: "Photos and videos from Pixabay".into(),
            attribution_url: "https://pixabay.com".into(),
        })
    }
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn hit(v: &Value, kind: StockKind) -> Option<StockHit> {
    let id = v.get("id")?.to_string();
    let tags = text(v, "tags");
    let creator = text(v, "user");
    let user_id = v.get("user_id").map(|u| u.to_string()).unwrap_or_default();
    let creator_url = if creator.is_empty() {
        String::new()
    } else {
        format!("https://pixabay.com/users/{creator}-{user_id}/")
    };
    let (thumb, preview, download, width, height, duration) = match kind {
        StockKind::Photo => (
            v.get("webformatURL")
                .and_then(Value::as_str)
                .map(str::to_string),
            None,
            v.get("largeImageURL")
                .or_else(|| v.get("webformatURL"))
                .and_then(Value::as_str)?
                .to_string(),
            v.get("imageWidth").and_then(Value::as_u64).unwrap_or(0) as u32,
            v.get("imageHeight").and_then(Value::as_u64).unwrap_or(0) as u32,
            0.0,
        ),
        StockKind::Video => {
            let videos = v.get("videos")?;
            // `large` is often empty (no 4K source); `medium` is 1080p.
            let pick = ["large", "medium", "small", "tiny"]
                .iter()
                .filter_map(|size| videos.get(*size))
                .find(|f| !text(f, "url").is_empty())?;
            let small = ["tiny", "small"]
                .iter()
                .filter_map(|size| videos.get(*size))
                .find(|f| !text(f, "url").is_empty());
            let thumb = ["medium", "small", "large", "tiny"]
                .iter()
                .filter_map(|size| videos.get(*size))
                .map(|f| text(f, "thumbnail"))
                .find(|t| !t.is_empty());
            (
                thumb,
                small.map(|f| text(f, "url")),
                text(pick, "url"),
                pick.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
                pick.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
                v.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
            )
        }
        StockKind::Sound => return None,
    };
    let noun = if kind == StockKind::Video {
        "Video"
    } else {
        "Image"
    };
    Some(StockHit {
        provider: "pixabay".into(),
        title: if tags.is_empty() {
            format!("Pixabay {id}")
        } else {
            tags
        },
        credit: format!("{noun} by {creator} from Pixabay"),
        creator,
        creator_url,
        source_url: text(v, "pageURL"),
        thumb_url: thumb,
        preview_url: preview,
        download_url: download,
        width,
        height,
        duration_seconds: duration,
        licence: licence(),
        kind,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::http::test_server;
    use super::super::super::registry::ProviderKind;
    use super::*;

    fn pixabay(url: &str) -> Pixabay {
        let mut account = Account::of_kind(ProviderKind::Pixabay);
        account.base_url = url.to_string();
        Pixabay::new(account, "pb-key-123".into())
    }

    #[test]
    fn videos_take_the_largest_file_there_is() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"total":1,"totalHits":30,"hits":[{"id":125,"pageURL":"https://pixabay.com/videos/id-125/","tags":"flowers, nature","duration":12,"videos":{"large":{"url":"","width":0,"height":0,"thumbnail":""},"medium":{"url":"https://cdn/m.mp4","width":1920,"height":1080,"thumbnail":"https://cdn/m.jpg"},"tiny":{"url":"https://cdn/t.mp4","width":640,"height":360,"thumbnail":"https://cdn/t.jpg"}},"user":"Coverr-Free-Footage","user_id":1281706}]}"#.to_vec(),
        )]);
        let page = pixabay(&server.url)
            .search(&StockQuery::new("flowers", StockKind::Video))
            .unwrap();
        assert!(page.has_more, "24 seen of 30");
        let hit = &page.hits[0];
        assert_eq!(hit.download_url, "https://cdn/m.mp4");
        assert_eq!(hit.preview_url.as_deref(), Some("https://cdn/t.mp4"));
        assert_eq!(hit.thumb_url.as_deref(), Some("https://cdn/m.jpg"));
        assert_eq!(hit.title, "flowers, nature");
        let sent = server.requests.lock().unwrap()[0].clone();
        assert!(
            sent.request_line
                .starts_with("GET /api/videos/?key=pb-key-123&q=flowers&per_page=24"),
            "{}",
            sent.request_line
        );
    }

    #[test]
    fn the_key_never_shows_in_an_error() {
        let server = test_server::serve(vec![(
            400,
            "text/plain",
            b"[ERROR 400] Invalid or missing API key (pb-key-123).".to_vec(),
        )]);
        let report = pixabay(&server.url).test();
        assert!(!report.ok);
        assert!(!report.message.contains("pb-key-123"), "{}", report.message);
    }
}
