//! Freesound: sound effects under Creative Commons licences.
//!
//! The API token (`Authorization: Token <key>`) is enough for search and for
//! the high-quality MP3 previews, which are what a short video needs; the
//! original files would need OAuth2. Each sound has its own licence: CC0,
//! CC BY, or CC BY-NC. Non-commercial sounds are filtered out on the server
//! and again here unless the user asks for them
//! (`docs/research/open-assets.md`, "Details: sound effects").

use serde_json::Value;

use super::super::http;
use super::super::provenance::Licence;
use super::super::registry::Account;
use super::super::stock::{self, StockHit, StockKind, StockPage, StockQuery, StockSearch};
use super::super::TestReport;

const FIELDS: &str = "id,name,username,license,previews,duration,url,images";

pub struct Freesound {
    account: Account,
    key: String,
}

impl Freesound {
    pub fn new(account: Account, key: String) -> Self {
        Self { account, key }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let response = http::agent()
            .get(&self.account.url(path))
            .set("Authorization", &format!("Token {}", self.key))
            .call()
            .map_err(|e| http::describe(e, &self.key))?;
        http::read_json(response)
    }

    pub fn test(&self) -> TestReport {
        match self.get("search/text/?query=rain&page_size=1&fields=id") {
            Ok(body) => TestReport::ok(format!(
                "Connected, {} sounds for \"rain\"",
                body.get("count").and_then(Value::as_u64).unwrap_or(0)
            )),
            Err(message) => TestReport::failed(message),
        }
    }
}

impl StockSearch for Freesound {
    fn kinds(&self) -> &'static [StockKind] {
        &[StockKind::Sound]
    }

    fn search(&self, query: &StockQuery) -> Result<StockPage, String> {
        if query.kind != StockKind::Sound {
            return Err("Freesound has sounds only".to_string());
        }
        let mut path = format!(
            "search/text/?query={}&page={}&page_size={}&fields={FIELDS}",
            http::encode(query.text.trim()),
            query.page.max(1),
            query.per_page.clamp(1, 150)
        );
        if !query.include_non_commercial {
            path.push_str("&filter=");
            path.push_str(&http::encode(
                "license:(\"Attribution\" OR \"Creative Commons 0\")",
            ));
        }
        let body = self.get(&path)?;
        let hits = body
            .get("results")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(sound).collect())
            .unwrap_or_default();
        Ok(StockPage {
            hits: stock::filter_licences(hits, query.include_non_commercial),
            total: body.get("count").and_then(Value::as_u64).unwrap_or(0),
            has_more: body.get("next").is_some_and(|n| !n.is_null()),
            attribution: "Sounds from Freesound".into(),
            attribution_url: "https://freesound.org".into(),
        })
    }
}

fn sound(v: &Value) -> Option<StockHit> {
    let id = v.get("id")?.to_string();
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let user = v
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let licence = Licence::from_cc_url(v.get("license").and_then(Value::as_str).unwrap_or(""));
    let previews = v.get("previews");
    let preview = |key: &str| {
        previews
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let download = preview("preview-hq-mp3").or_else(|| preview("preview-lq-mp3"))?;
    let page = v
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://freesound.org/s/{id}/"));
    // The credit format from Freesound's FAQ.
    let short = format!("freesound.org/s/{id}/");
    let credit = format!(
        "\"{name}\" by {user} ({short}) licensed under {}",
        licence.name
    );
    Some(StockHit {
        provider: "freesound".into(),
        title: name,
        creator_url: format!("https://freesound.org/people/{user}/"),
        creator: user,
        source_url: page,
        thumb_url: v
            .get("images")
            .and_then(|i| i.get("waveform_m"))
            .and_then(Value::as_str)
            .map(str::to_string),
        preview_url: preview("preview-lq-mp3").or_else(|| Some(download.clone())),
        download_url: download,
        width: 0,
        height: 0,
        duration_seconds: v.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
        licence,
        credit,
        kind: StockKind::Sound,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::http::test_server;
    use super::super::super::provenance::Commercial;
    use super::super::super::registry::ProviderKind;
    use super::*;

    fn freesound(url: &str) -> Freesound {
        let mut account = Account::of_kind(ProviderKind::Freesound);
        account.base_url = url.to_string();
        Freesound::new(account, "fs-token".into())
    }

    const ANSWER: &[u8] = br#"{"count":2,"next":null,"results":[
        {"id":401275,"name":"Rain, Moderate, C.wav","username":"InspectorJ","license":"https://creativecommons.org/licenses/by/4.0/","duration":60.5,"url":"https://freesound.org/people/InspectorJ/sounds/401275/","previews":{"preview-hq-mp3":"https://cdn/hq.mp3","preview-lq-mp3":"https://cdn/lq.mp3"},"images":{"waveform_m":"https://cdn/w.png"}},
        {"id":5,"name":"nc rain","username":"x","license":"http://creativecommons.org/licenses/by-nc/3.0/","previews":{"preview-hq-mp3":"https://cdn/5.mp3"}}
    ]}"#;

    #[test]
    fn non_commercial_sounds_are_hidden_by_default() {
        let server = test_server::serve(vec![(200, "application/json", ANSWER.to_vec())]);
        let page = freesound(&server.url)
            .search(&StockQuery::new("rain", StockKind::Sound))
            .unwrap();
        assert_eq!(
            page.hits.len(),
            1,
            "the NC one is dropped even if the server sent it"
        );
        let hit = &page.hits[0];
        assert_eq!(
            hit.credit,
            "\"Rain, Moderate, C.wav\" by InspectorJ (freesound.org/s/401275/) licensed under CC BY 4.0"
        );
        assert!(hit.licence.attribution_required);
        assert_eq!(hit.download_url, "https://cdn/hq.mp3");
        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.header("authorization"), Some("Token fs-token"));
        assert!(sent.request_line.contains("&filter=license%3A"));
    }

    #[test]
    fn they_show_when_asked_for() {
        let server = test_server::serve(vec![(200, "application/json", ANSWER.to_vec())]);
        let mut query = StockQuery::new("rain", StockKind::Sound);
        query.include_non_commercial = true;
        let page = freesound(&server.url).search(&query).unwrap();
        assert_eq!(page.hits.len(), 2);
        assert_eq!(page.hits[1].licence.commercial, Commercial::No);
        assert!(!server.requests.lock().unwrap()[0]
            .request_line
            .contains("filter="));
    }
}
