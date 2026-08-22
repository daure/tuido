use std::time::Duration;

use regex::Regex;

pub(crate) async fn fetch(url: &str) -> Option<String> {
    let target = crate::task_link::browser_target(url);
    if !target.starts_with("http://") && !target.starts_with("https://") {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Tuido link title fetcher")
        .build()
        .ok()?;
    let response = client.get(target).send().await.ok()?.error_for_status().ok()?;
    if response
        .content_length()
        .is_some_and(|length| length > 2 * 1024 * 1024)
    {
        return None;
    }
    extract(&response.text().await.ok()?)
}

fn extract(body: &str) -> Option<String> {
    let title = Regex::new(r"(?is)<title[^>]*>\s*(.*?)\s*</title>")
        .expect("title regex must compile")
        .captures(body)?
        .get(1)?
        .as_str();
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(decode_entities(&normalized))
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::extract;

    #[test]
    fn extracts_and_normalizes_html_titles() {
        assert_eq!(
            extract("<html><title>  Hello &amp;\n world  </title></html>"),
            Some("Hello & world".into())
        );
    }
}
