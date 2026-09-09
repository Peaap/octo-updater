use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use std::{io::Read, time::Duration};

const NEWS_FEED_URL: &str = "https://peaap.github.io/octo-updater/news.json";
const ALLOWED_HOSTS: &[&str] = &["peaap.github.io"];
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const USER_AGENT: &str = "OctoUpdater/1.3.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsPost {
    pub id: String,
    pub title: String,
    pub date: String,
    pub author: Option<String>,
    pub body: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFeed {
    pub featured: Option<NewsPost>,
    pub patch_notes: Vec<NewsPost>,
}

pub fn fetch_news() -> Result<NewsFeed, String> {
    let feed = download_json(NEWS_FEED_URL)?;
    validate_feed(&feed)?;
    Ok(feed)
}

fn download_json(url: &str) -> Result<NewsFeed, String> {
    let parsed = Url::parse(url).map_err(|error| format!("Invalid news URL: {error}"))?;
    validate_url(&parsed)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .redirect(Policy::custom(|attempt| {
            match validate_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(message) => attempt.error(message),
            }
        }))
        .build()
        .map_err(|error| format!("Could not build the news client: {error}"))?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| format!("Could not reach the launcher news feed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("The launcher news feed returned an error: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("The launcher news response is too large.".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = response
            .read(&mut chunk)
            .map_err(|error| format!("Could not read the launcher news feed: {error}"))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err("The launcher news response is too large.".into());
        }
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("The launcher news feed was invalid: {error}"))
}

fn validate_url(url: &Url) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err("Refusing a non-HTTPS launcher news URL.".into());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(format!("Refusing an unexpected news host: {host}"));
    }
    Ok(())
}

fn validate_feed(feed: &NewsFeed) -> Result<(), String> {
    if feed.patch_notes.len() > 12 {
        return Err("The launcher news feed contains too many patch notes.".into());
    }
    for post in feed.featured.iter().chain(&feed.patch_notes) {
        if post.id.trim().is_empty() || post.title.trim().is_empty() || post.date.trim().is_empty()
        {
            return Err("The launcher news feed contains an incomplete post.".into());
        }
        if let Some(url) = &post.url {
            let parsed = Url::parse(url).map_err(|error| {
                format!("The launcher news feed contains an invalid post URL: {error}")
            })?;
            if parsed.scheme() != "https" {
                return Err("The launcher news feed contains a non-HTTPS post URL.".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_feed() -> NewsFeed {
        NewsFeed {
            featured: Some(NewsPost {
                id: "announcement-1".into(),
                title: "Announcement".into(),
                date: "2026-09-09T12:00:00Z".into(),
                author: Some("OctoWoW".into()),
                body: "Plain text".into(),
                url: Some("https://octowow.st/forum/viewtopic.php?t=1".into()),
            }),
            patch_notes: vec![],
        }
    }

    #[test]
    fn rejects_untrusted_or_non_https_news_urls() {
        assert!(validate_url(
            &Url::parse("http://peaap.github.io/octo-updater/news.json").unwrap()
        )
        .is_err());
        assert!(validate_url(&Url::parse("https://example.com/news.json").unwrap()).is_err());
        assert!(validate_url(&Url::parse(NEWS_FEED_URL).unwrap()).is_ok());
    }

    #[test]
    fn accepts_a_complete_static_feed() {
        assert!(validate_feed(&sample_feed()).is_ok());
    }

    #[test]
    fn rejects_incomplete_or_non_https_posts() {
        let mut feed = sample_feed();
        feed.featured.as_mut().unwrap().title.clear();
        assert!(validate_feed(&feed).is_err());
        feed.featured.as_mut().unwrap().title = "Fixed".into();
        feed.featured.as_mut().unwrap().url = Some("http://example.com/post".into());
        assert!(validate_feed(&feed).is_err());
    }
}
