//! DeepL: caption translation.
//!
//! Every caption line is its own `text` value in one request, and the
//! answers come back in the same order, so timing never moves. Free keys end
//! in `:fx` and live on another host; the host is chosen from the key, never
//! asked for (`docs/research/integrations.md` §5.2).

use serde_json::{json, Value};

use super::super::http::{self, SendJson as _};
use super::super::registry::{descriptor, Account};
use super::super::{TestReport, Translate};

/// DeepL takes up to 50 texts per request.
const BATCH: usize = 50;

pub struct Deepl {
    account: Account,
    key: String,
}

impl Deepl {
    pub fn new(account: Account, key: String) -> Self {
        Self { account, key }
    }

    /// The account's host, or the free API's when the key is a free one and
    /// the account still points at the default host.
    fn url(&self, path: &str) -> String {
        let default = descriptor(self.account.kind).presets[0].1;
        let base = if self.account.base_url.trim_end_matches('/') == default
            && self.key.ends_with(":fx")
        {
            "https://api-free.deepl.com"
        } else {
            self.account.base_url.as_str()
        };
        http::join(base, path)
    }

    fn authorized(&self, request: ureq::Request) -> ureq::Request {
        request.set("Authorization", &format!("DeepL-Auth-Key {}", self.key))
    }

    /// `GET /v2/usage`: characters used of the limit.
    pub fn test(&self) -> TestReport {
        let response = self
            .authorized(http::agent().get(&self.url("v2/usage")))
            .call()
            .map_err(|e| http::describe(e, &self.key))
            .and_then(http::read_json);
        match response {
            Ok(body) => {
                let used = body.get("character_count").and_then(Value::as_u64);
                let limit = body.get("character_limit").and_then(Value::as_u64);
                let message = match (used, limit) {
                    (Some(used), Some(limit)) if limit > 0 => format!(
                        "Connected, {} of {} characters used",
                        group(used),
                        group(limit)
                    ),
                    _ => "Connected".to_string(),
                };
                TestReport::ok(message)
            }
            Err(message) => TestReport::failed(message),
        }
    }
}

/// `1234567` as `1,234,567`.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// DeepL's target codes: mostly the ISO code upper-cased, with a variant
/// required for English and Portuguese.
pub fn target_code(language: &str) -> String {
    match language.to_ascii_lowercase().as_str() {
        "en" => "EN-US".to_string(),
        "pt" => "PT-BR".to_string(),
        "zh" => "ZH-HANS".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

impl Translate for Deepl {
    fn translate(
        &self,
        lines: &[String],
        target: &str,
        source: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let mut out = Vec::with_capacity(lines.len());
        for chunk in lines.chunks(BATCH) {
            let mut body = json!({
                "text": chunk,
                "target_lang": target_code(target),
                "preserve_formatting": true,
            });
            if let Some(source) = source.filter(|s| !s.is_empty()) {
                body["source_lang"] = json!(source.to_ascii_uppercase());
            }
            let answer = self
                .authorized(http::agent().post(&self.url("v2/translate")))
                .send_json(body)
                .map_err(|e| http::describe(e, &self.key))
                .and_then(http::read_json)?;
            let translations: Vec<String> = answer
                .get("translations")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|t| {
                            t.get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();
            if translations.len() != chunk.len() {
                return Err(format!(
                    "DeepL returned {} lines for {}",
                    translations.len(),
                    chunk.len()
                ));
            }
            out.extend(translations);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::http::test_server;
    use super::super::super::registry::ProviderKind;
    use super::*;

    fn deepl(url: &str, key: &str) -> Deepl {
        let mut account = Account::of_kind(ProviderKind::Deepl);
        account.base_url = url.to_string();
        Deepl::new(account, key.to_string())
    }

    #[test]
    fn a_free_key_goes_to_the_free_host() {
        let free = Deepl::new(Account::of_kind(ProviderKind::Deepl), "abc:fx".into());
        assert_eq!(free.url("v2/usage"), "https://api-free.deepl.com/v2/usage");
        let pro = Deepl::new(Account::of_kind(ProviderKind::Deepl), "abc".into());
        assert_eq!(pro.url("v2/usage"), "https://api.deepl.com/v2/usage");
    }

    #[test]
    fn lines_are_translated_in_order() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"translations":[{"text":"Hallo"},{"text":"Welt"}]}"#.to_vec(),
        )]);
        let out = deepl(&server.url, "k")
            .translate(&["Hello".into(), "World".into()], "de", Some("en"))
            .unwrap();
        assert_eq!(out, ["Hallo", "Welt"]);
        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.request_line, "POST /v2/translate HTTP/1.1");
        assert_eq!(sent.header("authorization"), Some("DeepL-Auth-Key k"));
        let body: Value = serde_json::from_slice(&sent.body).unwrap();
        assert_eq!(body["target_lang"], "DE");
        assert_eq!(body["source_lang"], "EN");
        assert_eq!(body["text"][1], "World");
    }

    #[test]
    fn usage_is_the_connection_test() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"character_count":1200,"character_limit":500000}"#.to_vec(),
        )]);
        let report = deepl(&server.url, "k").test();
        assert!(report.ok);
        assert_eq!(
            report.message,
            "Connected, 1,200 of 500,000 characters used"
        );
        assert_eq!(target_code("en"), "EN-US");
    }
}
