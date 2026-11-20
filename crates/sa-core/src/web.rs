//! web fetch and search (spec 9).
//!
//! nothing here is a bare http client. the daemon speaks chrome's tls hello, http/2
//! settings, header order and header values, so a site that fingerprints the connection
//! sees a browser rather than a rust program. what it cannot do is run javascript, so a
//! page behind an interactive challenge is fetched as the challenge page itself.

use std::fmt::Write as _;
use std::time::Duration;

use serde_json::Value;
use wreq_util::Emulation;

use crate::tools::ToolError;

/// a page that never answers must not hold a turn.
const TIMEOUT: Duration = Duration::from_secs(30);

/// a page is a document, not a disk: past this the rest is dropped.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// results a search returns when the caller does not say.
pub const DEFAULT_RESULTS: usize = 5;

/// the cap on results, so one call cannot flood the model's context.
pub const MAX_RESULTS: usize = 20;

/// one search hit, as the search endpoint reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// the shared client: one connection pool and one cookie jar, so a session looks like one
/// browser rather than a new one per call.
fn client() -> Result<&'static wreq::Client, ToolError> {
    static CLIENT: std::sync::OnceLock<wreq::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = wreq::Client::builder()
        .emulation(Emulation::Chrome149)
        .cookie_store(true)
        // wreq follows no redirect by default; a browser follows ten.
        .redirect(wreq::redirect::Policy::limited(10))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| ToolError::Args(format!("web client: {e}")))?;
    Ok(CLIENT.get_or_init(|| built))
}

/// only http and https are fetched: `file:` in a tool would make the workspace rule a lie.
fn checked(url: &str) -> Result<&str, ToolError> {
    let url = url.trim();
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(url)
    } else {
        Err(ToolError::Args(format!(
            "{url}: only http and https are fetched"
        )))
    }
}

/// fetches a page and reduces it to readable markdown.
pub async fn fetch(url: &str) -> Result<String, ToolError> {
    let url = checked(url)?;
    let response = client()?
        .get(url)
        .send()
        .await
        .map_err(|e| ToolError::Args(format!("{url}: {e}")))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(wreq::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .text()
        .await
        .map_err(|e| ToolError::Args(format!("{url}: {e}")))?;
    let body = cap(body);

    // anything that is not markup is handed over as it came: a json api or a plain text
    // file has no article to extract.
    let markdown = if content_type.contains("html") || looks_like_html(&body) {
        readable(&body, url)
    } else {
        body
    };
    Ok(format!("url: {url}\nstatus: {status}\n\n{markdown}"))
}

/// searches the web and returns the hits as a numbered list.
pub async fn search(query: &str, limit: usize) -> Result<String, ToolError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ToolError::Args("web_search: empty query".to_owned()));
    }
    let limit = limit.clamp(1, MAX_RESULTS);
    let response = client()?
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| ToolError::Args(format!("web_search: {e}")))?;
    let html = response
        .text()
        .await
        .map_err(|e| ToolError::Args(format!("web_search: {e}")))?;
    let results = parse_results(&html, limit);
    if results.is_empty() {
        // the html endpoint answers with a challenge page when it does not like the
        // caller, and a challenge page has no results.
        return Ok(format!("no results for {query}"));
    }
    let mut out = format!("results for {query}:\n");
    for (index, result) in results.iter().enumerate() {
        let number = index + 1;
        let SearchResult {
            title,
            url,
            snippet,
        } = result;
        let _ = writeln!(out, "\n{number}. {title}\n   {url}\n   {snippet}");
    }
    Ok(out)
}

/// the hits in a duckduckgo html page, in the order it listed them.
fn parse_results(html: &str, limit: usize) -> Vec<SearchResult> {
    let doc = dom_query::Document::from(html);
    // one anchor and one snippet per result, emitted in the same order, so they pair by
    // index. a result without a snippet keeps an empty one rather than shifting the rest.
    let anchors = doc.select("a.result__a").nodes().to_vec();
    let snippets = doc.select("a.result__snippet").nodes().to_vec();
    let mut results = Vec::new();
    for (index, anchor) in anchors.iter().enumerate() {
        let Some(url) = anchor.attr("href").and_then(|href| target(&href)) else {
            continue;
        };
        let title = anchor.text().trim().to_owned();
        if title.is_empty() {
            continue;
        }
        let snippet = snippets
            .get(index)
            .map(|node| node.text().trim().to_owned())
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
        if results.len() >= limit {
            break;
        }
    }
    results
}

/// the destination behind a duckduckgo result link.
///
/// the endpoint wraps every hit in a redirect of its own, so the real url is the `uddg`
/// parameter; a link that is already direct is used as it stands.
fn target(href: &str) -> Option<String> {
    let href = href.trim();
    if let Some(rest) = href.split("uddg=").nth(1) {
        let encoded = rest.split('&').next().unwrap_or_default();
        let decoded = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .ok()?;
        return Some(decoded.into_owned());
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_owned());
    }
    // protocol-relative, as duckduckgo writes its own links.
    href.starts_with("//").then(|| format!("https:{href}"))
}

/// the readable part of a page, as markdown.
fn readable(html: &str, url: &str) -> String {
    let article = dom_smoothie::Readability::new(html, Some(url), None)
        .and_then(|mut readability| readability.parse())
        .ok();
    let (title, content) = match article {
        Some(article) => (article.title, article.content.to_string()),
        // nothing looked like an article: the whole page is better than nothing.
        None => (String::new(), html.to_owned()),
    };
    let markdown = htmd::convert(&content).unwrap_or(content);
    let markdown = markdown.trim();
    if title.trim().is_empty() {
        markdown.to_owned()
    } else {
        format!("# {}\n\n{markdown}", title.trim())
    }
}

/// keeps a body inside the cap without splitting a character.
fn cap(body: String) -> String {
    if body.len() <= MAX_BYTES {
        return body;
    }
    let mut end = MAX_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &body[..end])
}

fn looks_like_html(body: &str) -> bool {
    let head: String = body.chars().take(512).collect();
    let head = head.to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html")
}

/// the tool arguments, as the registry parses them.
pub fn limit_from(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_RESULTS, |limit| {
            usize::try_from(limit).unwrap_or(DEFAULT_RESULTS)
        })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        <div class="result results_links web-result">
          <h2 class="result__title">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F&amp;rut=abc">Rust Programming Language</a>
          </h2>
          <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F">A fast and reliable language.</a>
        </div>
        <div class="result results_links web-result">
          <h2 class="result__title">
            <a class="result__a" href="https://example.com/plain">Direct Link</a>
          </h2>
        </div>
    "#;

    #[test]
    fn results_pair_titles_with_their_real_targets() {
        let results = parse_results(PAGE, 10);
        assert_eq!(results.len(), 2, "{results:?}");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].snippet, "A fast and reliable language.");
        // a link that is already direct is not decoded again.
        assert_eq!(results[1].url, "https://example.com/plain");
        assert_eq!(
            results[1].snippet, "",
            "a missing snippet does not shift the rest"
        );
    }

    #[test]
    fn the_limit_is_respected() {
        assert_eq!(parse_results(PAGE, 1).len(), 1);
    }

    #[test]
    fn a_challenge_page_has_no_results() {
        let challenge = "<html><body><form id=\"challenge-form\"></form></body></html>";
        assert_eq!(parse_results(challenge, 10), Vec::new());
    }

    #[test]
    fn only_http_urls_are_fetched() {
        assert!(checked("https://example.com").is_ok());
        assert!(checked("http://example.com").is_ok());
        assert!(checked("ftp://example.com").is_err());
        assert!(checked("file:///etc/passwd").is_err());
    }

    #[test]
    fn an_article_becomes_markdown_without_its_scripts() {
        let page = r"
            <html><head><title>Notes</title></head><body>
            <script>var ignored = 1;</script>
            <article><h1>Notes</h1><p>The prose that matters.</p></article>
            </body></html>
        ";
        let markdown = readable(page, "https://example.com/notes");
        assert!(markdown.contains("The prose that matters."), "{markdown}");
        assert!(!markdown.contains("ignored"), "{markdown}");
    }

    #[test]
    fn an_oversized_body_is_capped_on_a_character_boundary() {
        let body = "é".repeat(MAX_BYTES);
        let capped = cap(body);
        assert!(capped.ends_with("[truncated]"), "the cut is marked");
        assert!(capped.len() <= MAX_BYTES + 16);
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn a_live_page_is_fetched_as_markdown() {
        let page = fetch("https://example.com").await.expect("fetch");
        assert!(page.contains("Example Domain"), "{page}");
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn a_live_search_returns_hits() {
        let results = search("rust programming language", 3)
            .await
            .expect("search");
        assert!(results.contains("rust-lang.org"), "{results}");
    }
}
