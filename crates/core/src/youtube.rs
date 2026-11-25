//! YouTube videos as sources: the captions YouTube shows, as a transcript
//! with a timestamp on each paragraph and a heading for each chapter the
//! description lists.

use std::time::Duration;

use serde::Deserialize;

use crate::{Error, Result, transcript};

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct Transcript {
    pub title: String,
    pub markdown: String,
}

struct AppClient {
    name: &'static str,
    version: &'static str,
    user_agent: &'static str,
}

/// YouTube's mobile apps. The web player withholds captions from clients
/// without a proof-of-origin token; these apps still get them. An app
/// version YouTube stops accepting is why there are two.
const CLIENTS: [AppClient; 2] = [
    AppClient {
        name: "ANDROID",
        version: "20.10.38",
        user_agent: "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip",
    },
    AppClient {
        name: "IOS",
        version: "20.10.4",
        user_agent: "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
    },
];

/// The 11-character video id in a watch, short, embed, Shorts, or live link.
pub fn video_id(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let id = match host {
        "youtu.be" => url.path_segments()?.next()?.to_string(),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            let mut segments = url.path_segments()?;
            match segments.next()? {
                "watch" => url.query_pairs().find(|(k, _)| k == "v")?.1.into_owned(),
                "shorts" | "embed" | "live" | "v" => segments.next()?.to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    let valid = id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then_some(id)
}

/// The playlist id in a YouTube playlist URL (`/playlist?list=...`).
/// Regular watch links that happen to carry `&list=` are left as single videos.
pub fn playlist_id(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    if !matches!(host, "youtube.com" | "music.youtube.com") {
        return None;
    }
    let first = url.path_segments()?.next()?;
    if first != "playlist" {
        return None;
    }
    let id = url.query_pairs().find(|(k, _)| k == "list")?.1.into_owned();
    let valid = (10..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then_some(id)
}

/// The videos of a public playlist.
pub struct Playlist {
    /// Watch addresses, in the playlist's order.
    pub videos: Vec<String>,
    /// YouTube's page couldn't be read, and the playlist's feed, which
    /// stood in, was full: the playlist may hold more videos than these.
    pub cut_short: bool,
}

/// Most videos taken from one playlist.
const MAX_PLAYLIST_VIDEOS: usize = 50;

/// How many videos a playlist's feed lists at most.
const FEED_VIDEOS: usize = 15;

/// A playlist's videos, up to 50, from its page. YouTube reshapes that page
/// now and then; when none of the shapes Bowerbird knows is in it, the
/// playlist's feed, which has kept one format for years, gives the first 15.
pub async fn playlist(list_id: &str) -> Result<Playlist> {
    let http = crate::http::client()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    let ids =
        extract_playlist_video_ids(&playlist_page(&http, list_id).await?, MAX_PLAYLIST_VIDEOS);
    let (ids, cut_short) = match ids.is_empty() {
        false => (ids, false),
        true => {
            let ids = playlist_feed(&http, list_id).await?;
            // A feed with room left lists the whole playlist.
            let full = ids.len() >= FEED_VIDEOS;
            (ids, full)
        }
    };
    if ids.is_empty() {
        return Err(Error::Invalid(
            "Bowerbird found no videos in this playlist. If it has some, YouTube has changed its pages and Bowerbird's reader is out of date: add the videos one by one for now.".into(),
        ));
    }
    Ok(Playlist {
        videos: ids
            .into_iter()
            .map(|id| format!("https://www.youtube.com/watch?v={id}"))
            .collect(),
        cut_short,
    })
}

const BROWSER_AGENT: &str =
    "Mozilla/5.0 (compatible; Bowerbird; +https://github.com/jolovicdev/bowerbird)";

async fn playlist_page(http: &reqwest::Client, list_id: &str) -> Result<String> {
    let url = format!("https://www.youtube.com/playlist?list={list_id}");
    get(http, &url, BROWSER_AGENT).await
}

/// The video ids in a playlist's Atom feed: its first 15 videos. A playlist
/// that is private, or isn't there, has no feed.
async fn playlist_feed(http: &reqwest::Client, list_id: &str) -> Result<Vec<String>> {
    let url = format!("https://www.youtube.com/feeds/videos.xml?playlist_id={list_id}");
    let response = http
        .get(url)
        .header(reqwest::header::USER_AGENT, BROWSER_AGENT)
        .send()
        .await
        .map_err(youtube_error)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::Invalid(
            "YouTube has no public playlist at this link.".into(),
        ));
    }
    let feed = response
        .error_for_status()
        .map_err(youtube_error)?
        .text()
        .await
        .map_err(youtube_error)?;
    Ok(ids_between(&feed, PLAYLIST_FEED))
}

/// What stands before and after a video id in a playlist page. The page lists
/// each video as a "lockup" of the video type; its older layout, which some
/// visitors still get, used `playlistVideoRenderer`.
const PLAYLIST_LOCKUP: (&str, &str) = (
    "\"contentId\":\"",
    "\",\"contentType\":\"LOCKUP_CONTENT_TYPE_VIDEO\"",
);
const PLAYLIST_RENDERER: (&str, &str) = ("\"playlistVideoRenderer\":{\"videoId\":\"", "\"");
/// The same in a playlist's feed.
const PLAYLIST_FEED: (&str, &str) = ("<yt:videoId>", "</yt:videoId>");

fn extract_playlist_video_ids(html: &str, limit: usize) -> Vec<String> {
    let mut ids = ids_between(html, PLAYLIST_LOCKUP);
    if ids.is_empty() {
        ids = ids_between(html, PLAYLIST_RENDERER);
    }
    ids.truncate(limit);
    ids
}

/// Each video id found between `before` and `after`, once, in page order.
fn ids_between(html: &str, (before, after): (&str, &str)) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for rest in html.split(before).skip(1) {
        if let Some(id) = rest.get(..11)
            && rest[11..].starts_with(after)
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !ids.iter().any(|known| known == id)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

pub async fn transcript(id: &str) -> Result<Transcript> {
    let http = crate::http::client()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    let mut refusal = None;
    for app in &CLIENTS {
        match transcript_as(&http, app, id).await? {
            Attempt::Read(transcript) => return Ok(transcript),
            Attempt::Refused(e) => {
                refusal.get_or_insert(e);
            }
        }
    }
    Err(refusal.unwrap_or_else(|| Error::Http("Couldn't reach YouTube.".into())))
}

/// What one of YouTube's apps got for a video.
enum Attempt {
    Read(Transcript),
    /// This app was turned away, which says nothing about the next one.
    Refused(Error),
}

/// Reads a video's captions as one app. An error is about the video, such
/// as one without captions, and would be the same for any app.
async fn transcript_as(http: &reqwest::Client, app: &AppClient, id: &str) -> Result<Attempt> {
    let player: Player = match player(http, app, id).await {
        Ok(player) => player,
        Err(e) => return Ok(Attempt::Refused(e)),
    };
    let status = player.playability_status.unwrap_or_default();
    if status.status != "OK" {
        return Ok(Attempt::Refused(Error::Invalid(format!(
            "YouTube won't show this video to Bowerbird{}. Only public videos can be added.",
            status
                .reason
                .map(|r| format!(" (“{r}”)"))
                .unwrap_or_default()
        ))));
    }
    let details = player.video_details.unwrap_or_default();
    let tracks = player.captions.map(|c| c.list).unwrap_or_default();
    let Some(track) = choose_track(&tracks) else {
        return Err(Error::Invalid(
            "This video has no captions, so there's no text for Bowerbird to read. If you have the audio or video file, drop it in to transcribe it locally.".into(),
        ));
    };
    let raw = get(http, &json3_url(&track.base_url)?, app.user_agent).await?;
    if raw.trim().is_empty() {
        return Ok(Attempt::Refused(Error::Http(
            "YouTube didn't send this video's captions. Try again later.".into(),
        )));
    }
    let captions: Json3 = serde_json::from_str(&raw)
        .map_err(|e| Error::Http(format!("YouTube sent captions Bowerbird can't read: {e}")))?;
    let chapters = chapters(&details.short_description);
    Ok(Attempt::Read(Transcript {
        markdown: transcript::markdown(
            &details.title,
            &match details.author.trim() {
                "" => String::new(),
                author => format!("A video by {author} on YouTube."),
            },
            &captions
                .events
                .iter()
                .map(|e| {
                    (
                        e.t_start_ms / 1000,
                        e.segs.iter().map(|s| s.utf8.as_str()).collect(),
                    )
                })
                .collect::<Vec<_>>(),
            &chapters,
        ),
        title: details.title,
    }))
}

async fn player(http: &reqwest::Client, app: &AppClient, id: &str) -> Result<Player> {
    let body = serde_json::json!({
        "context": { "client": { "clientName": app.name, "clientVersion": app.version, "hl": "en" } },
        "videoId": id,
    })
    .to_string();
    let raw = crate::retry::with_retries(
        || async {
            http.post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
                .header(reqwest::header::USER_AGENT, app.user_agent)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
        },
        |e: &reqwest::Error| crate::retry::transient_status(e.status().map(|s| s.as_u16())),
    )
    .await
    .map_err(youtube_error)?
    .text()
    .await
    .map_err(youtube_error)?;
    serde_json::from_str(&raw).map_err(|e| {
        Error::Http(format!(
            "YouTube answered in a way Bowerbird can't read: {e}"
        ))
    })
}

async fn get(http: &reqwest::Client, url: &str, user_agent: &str) -> Result<String> {
    crate::retry::with_retries(
        || async {
            http.get(url)
                .header(reqwest::header::USER_AGENT, user_agent)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
        },
        |e: &reqwest::Error| crate::retry::transient_status(e.status().map(|s| s.as_u16())),
    )
    .await
    .map_err(youtube_error)?
    .text()
    .await
    .map_err(youtube_error)
}

fn youtube_error(e: reqwest::Error) -> Error {
    Error::Http(if e.is_timeout() {
        format!(
            "YouTube took longer than {} seconds to answer.",
            TIMEOUT.as_secs()
        )
    } else if let Some(status) = e.status() {
        format!("YouTube answered with an error ({status}).")
    } else {
        "Couldn't connect to YouTube. Check your internet connection.".into()
    })
}

/// The caption track's address, asking for the JSON format whatever it named.
fn json3_url(base: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(base)
        .map_err(|e| Error::Http(format!("YouTube sent a broken captions address: {e}")))?;
    let query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "fmt")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(query)
        .append_pair("fmt", "json3");
    Ok(url.into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Player {
    playability_status: Option<Playability>,
    video_details: Option<Details>,
    captions: Option<Captions>,
}

#[derive(Deserialize, Default)]
struct Playability {
    status: String,
    reason: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Details {
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    short_description: String,
}

#[derive(Deserialize)]
struct Captions {
    #[serde(rename = "playerCaptionsTracklistRenderer")]
    list: TrackList,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
struct TrackList {
    #[serde(default)]
    caption_tracks: Vec<Track>,
    /// One per audio language; a dubbed video has several.
    #[serde(default)]
    audio_tracks: Vec<AudioTrack>,
    /// The original audio.
    default_audio_track_index: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioTrack {
    /// Caption tracks for this audio, the one that matches it first.
    #[serde(default)]
    caption_track_indices: Vec<usize>,
    default_caption_track_index: Option<usize>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Track {
    base_url: String,
    language_code: String,
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Json3 {
    #[serde(default)]
    events: Vec<CaptionEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptionEvent {
    #[serde(default)]
    t_start_ms: u64,
    #[serde(default)]
    segs: Vec<Seg>,
}

#[derive(Deserialize)]
struct Seg {
    #[serde(default)]
    utf8: String,
}

/// The track YouTube pairs with the original audio, or a track someone
/// wrote in that language when YouTube's is the automatic one. A written
/// track in another language is a translation, and a dubbed video has an
/// automatic track for every dub.
fn choose_track(list: &TrackList) -> Option<&Track> {
    let tracks = &list.caption_tracks;
    let automatic = |t: &&Track| t.kind.as_deref() == Some("asr");
    let original = list
        .audio_tracks
        .get(list.default_audio_track_index.unwrap_or(0))
        .and_then(|a| {
            a.default_caption_track_index
                .or_else(|| a.caption_track_indices.first().copied())
        })
        .and_then(|i| tracks.get(i))
        .or_else(|| tracks.iter().find(automatic));
    let primary = |code: &str| code.split('-').next().unwrap_or(code).to_string();
    let mut written = tracks.iter().filter(|t| !automatic(t));
    match original {
        Some(spoken) if automatic(&spoken) => written
            .find(|t| primary(&t.language_code) == primary(&spoken.language_code))
            .or(Some(spoken)),
        Some(spoken) => Some(spoken),
        None => written.next(),
    }
}

/// Chapters listed in a description as "0:00 Title" lines, from the start.
fn chapters(description: &str) -> Vec<(u64, String)> {
    let mut found: Vec<(u64, String)> = Vec::new();
    for line in description.lines() {
        let line = line.trim();
        let Some((stamp, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Some(seconds) = clock_seconds(stamp) else {
            continue;
        };
        let title = rest.trim_start_matches(|c: char| c.is_whitespace() || "-–—:|".contains(c));
        if title.is_empty() || found.last().is_some_and(|(at, _)| seconds <= *at) {
            continue;
        }
        found.push((seconds, title.trim().to_string()));
    }
    if found.len() < 2 || found[0].0 != 0 {
        return Vec::new();
    }
    found
}

/// "1:02:03" or "12:34" as seconds.
fn clock_seconds(stamp: &str) -> Option<u64> {
    let parts: Vec<&str> = stamp.split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty() || p.len() > 2) {
        return None;
    }
    parts
        .iter()
        .try_fold(0u64, |total, p| Some(total * 60 + p.parse::<u64>().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_video_id_in_every_kind_of_link() {
        let id = Some("dQw4w9WgXcQ".to_string());
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "https://m.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://youtube.com/live/dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(url), id, "{url}");
        }
        for url in [
            "https://www.youtube.com/@channel",
            "https://www.youtube.com/watch?v=short",
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(url), None, "{url}");
        }
    }

    fn track(language: &str, kind: Option<&str>) -> Track {
        Track {
            base_url: format!("https://www.youtube.com/api/timedtext?lang={language}"),
            language_code: language.into(),
            kind: kind.map(str::to_string),
        }
    }

    fn list(
        tracks: Vec<Track>,
        audio: Vec<(Vec<usize>, Option<usize>)>,
        default: Option<usize>,
    ) -> TrackList {
        TrackList {
            caption_tracks: tracks,
            audio_tracks: audio
                .into_iter()
                .map(
                    |(caption_track_indices, default_caption_track_index)| AudioTrack {
                        caption_track_indices,
                        default_caption_track_index,
                    },
                )
                .collect(),
            default_audio_track_index: default,
        }
    }

    fn chosen(list: &TrackList) -> Option<String> {
        choose_track(list).map(|t| {
            format!(
                "{} {}",
                t.language_code,
                t.kind.as_deref().unwrap_or("written")
            )
        })
    }

    #[test]
    fn prefers_written_captions_in_the_spoken_language() {
        let tracks = || {
            vec![
                track("de-DE", None),
                track("en", Some("asr")),
                track("en-GB", None),
            ]
        };
        assert_eq!(
            chosen(&list(tracks(), vec![], None)).as_deref(),
            Some("en-GB written")
        );
        let youtube_default = list(tracks(), vec![(vec![0, 1, 2], Some(0))], Some(0));
        assert_eq!(chosen(&youtube_default).as_deref(), Some("de-DE written"));
        let translation = list(
            vec![track("de", None), track("en", Some("asr"))],
            vec![],
            None,
        );
        assert_eq!(chosen(&translation).as_deref(), Some("en asr"));
        assert_eq!(
            chosen(&list(vec![track("fr", None)], vec![], None)).as_deref(),
            Some("fr written")
        );
        assert_eq!(chosen(&TrackList::default()), None);
    }

    #[test]
    fn a_dubbed_video_uses_the_captions_of_its_original_audio() {
        let tracks = vec![
            track("ar", Some("asr")),
            track("bn", Some("asr")),
            track("en", Some("asr")),
        ];
        let dubbed = list(
            tracks,
            vec![(vec![0, 1, 2], None), (vec![2, 0, 1], None)],
            Some(1),
        );
        assert_eq!(chosen(&dubbed).as_deref(), Some("en asr"));
    }

    #[test]
    fn asks_for_json_captions() {
        let url = json3_url("https://www.youtube.com/api/timedtext?v=x&fmt=srv3&lang=en").unwrap();
        assert_eq!(
            url,
            "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=json3"
        );
    }

    #[test]
    fn reads_chapters_only_when_they_start_at_zero() {
        let description = "A lecture.\n\n0:00 Introduction\n2:05 - The window\n1:02:03 Aftermath: Westphalia\n1:00:00 out of order\nhttps://example.com";
        assert_eq!(
            chapters(description),
            [
                (0, "Introduction".to_string()),
                (125, "The window".to_string()),
                (3723, "Aftermath: Westphalia".to_string()),
            ]
        );
        assert!(chapters("1:00 Late start\n2:00 Next").is_empty());
        assert!(chapters("0:00 Only one").is_empty());
    }

    #[test]
    fn recognizes_playlist_urls_and_extracts_unique_video_ids() {
        assert_eq!(
            playlist_id("https://www.youtube.com/playlist?list=PL1234567890abcd").as_deref(),
            Some("PL1234567890abcd")
        );
        assert_eq!(
            playlist_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL1234567890abcd"),
            None
        );
        // Cut from a playlist page as YouTube served it: two videos, one of
        // them listed twice, and a lockup that is not a video.
        let page = r#"{"lockupViewModel":{"contentImage":{"thumbnailViewModel":{"image":{"sources":[{"url":"https://i.ytimg.com/vi/HtSuA80QTyo/hqdefault.jpg"}]}}},"metadata":{"lockupMetadataViewModel":{"title":{"content":"Lecture 1"}}},"contentId":"HtSuA80QTyo","contentType":"LOCKUP_CONTENT_TYPE_VIDEO","rendererContext":{}}},{"lockupViewModel":{"contentId":"Zc54gFhdpLA","contentType":"LOCKUP_CONTENT_TYPE_VIDEO","rendererContext":{}}},{"lockupViewModel":{"contentId":"HtSuA80QTyo","contentType":"LOCKUP_CONTENT_TYPE_VIDEO"}},{"lockupViewModel":{"contentId":"PLUl4u3cNGP61Oq3tWYp6V_F-5jb5L2iHb","contentType":"LOCKUP_CONTENT_TYPE_PLAYLIST"}}"#;
        assert_eq!(
            extract_playlist_video_ids(page, 10),
            ["HtSuA80QTyo", "Zc54gFhdpLA"]
        );
        assert_eq!(extract_playlist_video_ids(page, 1), ["HtSuA80QTyo"]);
        let older = r#"{"playlistVideoRenderer":{"videoId":"abcdefghijk"}},{"playlistVideoRenderer":{"videoId":"abcdefghijk"}},{"playlistVideoRenderer":{"videoId":"1234567890_"}}"#;
        assert_eq!(
            extract_playlist_video_ids(older, 10),
            ["abcdefghijk", "1234567890_"]
        );
    }

    #[test]
    fn a_playlist_feed_gives_its_video_ids_in_order() {
        // Two entries cut from a feed as YouTube served it.
        let feed = r#"<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015" xmlns="http://www.w3.org/2005/Atom">
 <yt:playlistId>PLUl4u3cNGP61Oq3tWYp6V_F-5jb5L2iHb</yt:playlistId>
 <entry>
  <id>yt:video:HtSuA80QTyo</id>
  <yt:videoId>HtSuA80QTyo</yt:videoId>
  <title>Lecture 1: Algorithmic Thinking, Peak Finding</title>
  <link rel="alternate" href="https://www.youtube.com/watch?v=HtSuA80QTyo"/>
 </entry>
 <entry>
  <id>yt:video:Zc54gFhdpLA</id>
  <yt:videoId>Zc54gFhdpLA</yt:videoId>
  <title>Lecture 2: Models of Computation, Document Distance</title>
 </entry>
</feed>"#;
        assert_eq!(
            ids_between(feed, PLAYLIST_FEED),
            ["HtSuA80QTyo", "Zc54gFhdpLA"]
        );
    }

    fn live() -> bool {
        let on = std::env::var("BOWERBIRD_LIVE").as_deref() == Ok("1");
        if !on {
            eprintln!("skipped: set BOWERBIRD_LIVE=1 to call YouTube");
        }
        on
    }

    fn http() -> reqwest::Client {
        crate::http::client().timeout(TIMEOUT).build().unwrap()
    }

    /// Calls YouTube, so it runs only with BOWERBIRD_LIVE=1. Each app is
    /// tried by itself: while one still gets captions, `transcript` hides
    /// that the other has stopped.
    #[tokio::test]
    async fn each_app_still_reads_a_video_s_captions() {
        if !live() {
            return;
        }
        for app in &CLIENTS {
            match transcript_as(&http(), app, "dQw4w9WgXcQ").await {
                Ok(Attempt::Read(transcript)) => assert!(
                    transcript.markdown.to_lowercase().contains("never gonna"),
                    "{} got captions without the lyrics",
                    app.name
                ),
                Ok(Attempt::Refused(e)) => panic!("YouTube turned {} away: {e}", app.name),
                Err(e) => panic!("{} failed: {e}", app.name),
            }
        }
    }

    /// Calls YouTube, so it runs only with BOWERBIRD_LIVE=1. The feed is what
    /// playlists fall back on, so it is checked while the page still works.
    #[tokio::test]
    async fn the_playlist_feed_still_lists_videos() {
        if !live() {
            return;
        }
        let ids = playlist_feed(&http(), "PLUl4u3cNGP61Oq3tWYp6V_F-5jb5L2iHb")
            .await
            .unwrap();
        assert!(ids.len() >= 10, "{ids:?}");
        let missing = playlist_feed(&http(), "PLxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").await;
        assert_eq!(
            missing.unwrap_err().to_string(),
            "YouTube has no public playlist at this link."
        );
    }
}
