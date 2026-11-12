use std::{env, time::Duration};

use super::*;

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";
const MAX_WEB_RESULTS: usize = 10;

/// Search the web with Tavily and format the top results.
///
/// Uses a blocking HTTP client because tool execution runs on
/// `spawn_blocking`, not on the async runtime.
pub(super) fn web_search(arguments: &str, api_key: Option<&str>) -> Result<String> {
    let Some(api_key) = api_key.map(str::trim).filter(|key| !key.is_empty()) else {
        bail!(
            "web search is not configured; add tavily_api_key to the config or set \
             TAVILY_API_KEY, then restart yopes"
        );
    };
    let args: WebSearchArgs = parse(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        bail!("query cannot be empty");
    }
    let max_results = args.max_results.unwrap_or(5).clamp(1, MAX_WEB_RESULTS);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .context("creating the search client")?;
    let response = client
        .post(TAVILY_SEARCH_URL)
        .bearer_auth(api_key)
        .json(&json!({ "query": query, "max_results": max_results }))
        .send()
        .context("contacting Tavily")?;
    let status = response.status();
    let body: Value = response.json().context("reading the Tavily response")?;
    if !status.is_success() {
        let message = body
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no detail");
        bail!("Tavily returned {status}: {message}");
    }
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .context("the Tavily response has no results")?;
    if results.is_empty() {
        return Ok(format!("{query}\n(no results)"));
    }
    let mut output = format!("{query}\n");
    for (index, result) in results.iter().enumerate() {
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let snippet = result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        output.push_str(&format!(
            "\n{}. {title}\n{url}\n{}\n",
            index + 1,
            snippet.trim()
        ));
    }
    Ok(truncate_output(output))
}

/// How many bytes one `web_fetch` may pull.
const MAX_FETCH_BYTES: usize = 1_000_000;

#[derive(Deserialize)]
struct WebFetchArgs {
    url: String,
}

/// Fetch one page over HTTP GET and return its text: scripts, styles and
/// tags are gone, whitespace is squeezed to lines. Blocking, because tool
/// execution runs on `spawn_blocking`.
pub(super) fn web_fetch(arguments: &str) -> Result<String> {
    let args: WebFetchArgs = parse(arguments)?;
    let url = args.url.trim();
    if url.is_empty() {
        bail!("url cannot be empty");
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("url must start with http:// or https://");
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .context("creating the fetch client")?;
    let response = client
        .get(url)
        .header("user-agent", format!("yopes/{}", env!("CARGO_PKG_VERSION")))
        .send()
        .with_context(|| format!("fetching {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("the server returned {status} for {url}");
    }
    let mut body_bytes = Vec::new();
    response
        .take((MAX_FETCH_BYTES + 1) as u64)
        .read_to_end(&mut body_bytes)
        .context("reading the response body")?;
    let body = String::from_utf8_lossy(&body_bytes);
    Ok(truncate_output(strip_html(&body)))
}

/// The page's text: `<script>` and `<style>` bodies dropped wholesale, all
/// other tags replaced by a space, the usual entities decoded, blank runs
/// collapsed. Good enough to read, deliberately not a browser engine.
pub(super) fn strip_html(body: &str) -> String {
    let body = drop_block(body, "script");
    let body = drop_block(&body, "style");
    let mut text = String::with_capacity(body.len());
    let mut at = 0;
    while at < body.len() {
        match body[at..].find('<') {
            None => {
                text.push_str(&body[at..]);
                break;
            }
            Some(open_rel) => {
                let open = at + open_rel;
                text.push_str(&body[at..open]);
                let Some(close_rel) = body[open..].find('>') else {
                    break;
                };
                // Tags often sit between words; a space keeps them separate.
                text.push(' ');
                at = open + close_rel + 1;
            }
        }
    }
    let text = decode_entities(&text);
    let mut clean = String::new();
    let mut blanks = 0;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        blanks = if line.is_empty() { blanks + 1 } else { 0 };
        if blanks <= 1 {
            clean.push_str(&line);
            clean.push('\n');
        }
    }
    clean.trim_end().to_owned()
}

/// Drop `<tag …>…</tag>` blocks: real pages nest scripts and styles everywhere
/// and their bodies are noise, not content.
fn drop_block(body: &str, tag: &str) -> String {
    let open_marker = format!("<{tag}");
    let close_marker = format!("</{tag}");
    let mut text = body.to_owned();
    loop {
        let lower = text.to_ascii_lowercase();
        let Some(start) = find_tag(&lower, &open_marker) else {
            return text;
        };
        let Some(close) = find_tag(&lower[start..], &close_marker).map(|rel| start + rel) else {
            // An unterminated block swallows everything after it.
            text.truncate(start);
            return text;
        };
        let end = close
            + close_marker.len()
            + lower[close + close_marker.len()..]
                .find('>')
                .map_or(0, |rel| rel + 1);
        let mut combined = String::with_capacity(text.len());
        combined.push_str(&text[..start]);
        combined.push(' ');
        combined.push_str(&text[end..]);
        text = combined;
    }
}

/// The offset of `<name` where the next character really opens an element:
/// `>`, `/` or whitespace, so `<scripts>` is never mistaken for `<script>`.
fn find_tag(hay: &str, marker: &str) -> Option<usize> {
    let mut at = 0;
    while let Some(rel) = hay[at..].find(marker) {
        let start = at + rel;
        at = start + marker.len();
        match hay[at..].chars().next() {
            Some('>') | Some('/') => return Some(start),
            Some(c) if c.is_ascii_whitespace() => return Some(start),
            _ => {}
        }
    }
    None
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    // `&amp;` last, so encoded entities stay encoded.
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}
