//! Reading web pages the user pasted, such as a student's profile, as plain text for Claude.
//!
//! Only links from the user's own message can be read, and never addresses on this computer
//! or the local network, so a page can't make the app reach anything the user didn't give.

use std::time::Duration;

use serde::Serialize;

const MAX_BYTES: usize = 3_000_000;
const MAX_TEXT: usize = 15_000;

/// The http(s) links in a message, in order, without trailing punctuation.
pub fn urls_in(text: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == '<' || c == '>') {
        let Some(start) = word.find("http://").or_else(|| word.find("https://")) else { continue };
        let url = word[start..].trim_end_matches(['.', ',', ';', ':', ')', ']', '"', '\'', '!', '?']);
        if url.len() > "https://".len() && !urls.iter().any(|u| u == url) {
            urls.push(url.to_string());
        }
    }
    urls
}

/// Refuses links that aren't http(s) or point at this computer or a private network.
pub fn check_public(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("\"{url}\" isn't a valid link"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http and https links can be read".into());
    }
    let host = parsed.host_str().unwrap_or_default().trim_matches(['[', ']']).to_lowercase();
    let private = host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".localhost")
        || match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified(),
            Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00,
            Err(_) => false,
        };
    if private {
        return Err("links to this computer or a local network can't be read".into());
    }
    Ok(parsed)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LinkPage {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Email addresses on the page, including "name [at] domain [dot] edu" forms.
    pub emails: Vec<String>,
    pub text: String,
}

/// Downloads a page and extracts its text. Blocks; call it off the async runtime.
pub fn read(url: &str, allow_private: bool) -> Result<LinkPage, String> {
    let html = fetch(url, allow_private)?;
    let mut page = extract(&html);
    page.url = url.to_string();
    Ok(page)
}

/// Downloads a public link's body as text. Blocks; call it off the async runtime.
pub fn fetch(url: &str, allow_private: bool) -> Result<String, String> {
    let parsed = if allow_private {
        reqwest::Url::parse(url).map_err(|e| e.to_string())?
    } else {
        check_public(url)?
    };
    tauri::async_runtime::block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("Mozilla/5.0 (Macintosh) Suk/0.1")
            .build()
            .map_err(|e| e.to_string())?;
        let response = client.get(parsed).send().await.map_err(|e| format!("couldn't open the link: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            let hint = if url.contains("linkedin.com") {
                " LinkedIn doesn't let apps read profiles; a university or personal page works better."
            } else {
                ""
            };
            return Err(format!("the site answered {status}.{hint}"));
        }
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("the page is too large to read".into());
        }
        Ok::<_, String>(String::from_utf8_lossy(&bytes).into_owned())
    })
}

/// The readable parts of an HTML page.
pub fn extract(html: &str) -> LinkPage {
    let cleaned = remove_blocks(html, &["script", "style", "noscript", "svg", "head"]);
    let head = html.split_once("</head>").map_or("", |(h, _)| h);
    let title = between(head, "<title", "</title>")
        .and_then(|t| t.split_once('>').map(|(_, t)| decode(t).trim().to_string()))
        .filter(|t| !t.is_empty());
    let description = meta_content(head, "description").or_else(|| meta_content(head, "og:description"));

    let mut emails: Vec<String> = Vec::new();
    let lower = cleaned.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("mailto:") {
        let address: String = rest[i + 7..].chars().take_while(|c| !"\"'?> ".contains(*c)).collect();
        push_email(&mut emails, &decode(&address));
        rest = &rest[i + 7..];
    }

    let mut text = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in cleaned.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_alphanumeric()).collect();
                if matches!(name.to_lowercase().as_str(), "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article" | "td" | "dt" | "dd") {
                    text.push('\n');
                } else {
                    text.push(' ');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => text.push(c),
        }
    }
    let text = decode(&text);
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() && lines.last() != Some(&line) {
            lines.push(line);
        }
    }
    let mut text = lines.join("\n");
    for word in text.replace(" [at] ", "@").replace(" (at) ", "@").replace("[at]", "@").replace(" [dot] ", ".").replace("[dot]", ".").split_whitespace() {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric());
        if word.contains('@') {
            push_email(&mut emails, word);
        }
    }
    if text.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(" …(cut)");
    }
    LinkPage { url: String::new(), title, description, emails, text }
}

fn push_email(emails: &mut Vec<String>, candidate: &str) {
    let candidate = candidate.trim().trim_matches('.').to_lowercase();
    let valid = candidate
        .split_once('@')
        .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.contains('@'))
        && candidate.chars().all(|c| c.is_alphanumeric() || "@._+-".contains(c));
    if valid && !emails.contains(&candidate) {
        emails.push(candidate);
    }
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let lower = text.to_lowercase();
    let from = lower.find(start)? + start.len();
    let to = from + lower[from..].find(end)?;
    text.get(from..to)
}

fn meta_content(head: &str, name: &str) -> Option<String> {
    let lower = head.to_lowercase();
    let mut offset = 0;
    while let Some(i) = lower[offset..].find("<meta") {
        let start = offset + i;
        let end = start + lower[start..].find('>')?;
        let tag = &head[start..end];
        let tag_lower = &lower[start..end];
        let names = [format!("name=\"{name}\""), format!("property=\"{name}\""), format!("name='{name}'")];
        if names.iter().any(|n| tag_lower.contains(n)) {
            let content = tag_lower.find("content=")?;
            let value = &tag[content + 8..];
            let quote = value.chars().next()?;
            let value = value[1..].split(quote).next()?;
            let value = decode(value).trim().to_string();
            return (!value.is_empty()).then_some(value);
        }
        offset = end;
    }
    None
}

/// Removes elements like <script>…</script> and HTML comments.
fn remove_blocks(html: &str, names: &[&str]) -> String {
    let mut out = html.to_string();
    while let (Some(start), Some(end)) = (out.find("<!--"), out.find("-->")) {
        if end < start {
            break;
        }
        out.replace_range(start..end + 3, " ");
    }
    for name in names {
        loop {
            let lower = out.to_lowercase();
            let Some(start) = lower.find(&format!("<{name}")) else { break };
            let close = format!("</{name}>");
            let end = lower[start..].find(&close).map_or(out.len(), |i| start + i + close.len());
            out.replace_range(start..end, " ");
        }
    }
    out
}

pub fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(10)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" | "lsquo" => Some('\''),
            "ldquo" | "rdquo" => Some('"'),
            _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32),
            _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_in_a_message() {
        assert_eq!(
            urls_in("Satya's page: https://iitg.ac.in/~satya/. Also (http://scholar.google.com/citations?user=abc)"),
            vec!["https://iitg.ac.in/~satya/", "http://scholar.google.com/citations?user=abc"]
        );
        assert!(urls_in("no links, just https:// and text").is_empty());
    }

    #[test]
    fn refuses_local_and_odd_links() {
        for bad in ["http://localhost:1420/", "http://127.0.0.1:5000/mcp", "http://192.168.1.4/", "http://[::1]/", "file:///etc/passwd", "http://printer.local/", "not a url"] {
            assert!(check_public(bad).is_err(), "{bad}");
        }
        assert!(check_public("https://www.iitg.ac.in/cse/faculty").is_ok());
    }

    #[test]
    fn extracts_text_title_and_emails_from_a_profile() {
        let html = r#"<!DOCTYPE html><html><head><title>Satya Prakash Das &mdash; IIT Guwahati</title>
            <meta name="description" content="PhD scholar working on fuzzing &amp; compilers">
            <style>.x { color: red }</style><script>var email = "fake@spam.com";</script></head>
            <body><!-- nav --><nav>Home</nav><h1>Satya Prakash&nbsp;Das</h1>
            <p>PhD Scholar, Department of CSE<br>IIT Guwahati</p>
            <p>Email: satya.das [at] example [dot] edu</p>
            <a href="mailto:SATYA@example.edu">Mail</a><ul><li>Fuzzing</li><li>Compilers &#x2014; LLVM</li></ul></body></html>"#;
        let page = extract(html);
        assert_eq!(page.title.as_deref(), Some("Satya Prakash Das — IIT Guwahati"));
        assert_eq!(page.description.as_deref(), Some("PhD scholar working on fuzzing & compilers"));
        assert_eq!(page.emails, vec!["satya@example.edu", "satya.das@example.edu"]);
        assert_eq!(
            page.text,
            "Home\nSatya Prakash Das\nPhD Scholar, Department of CSE\nIIT Guwahati\nEmail: satya.das [at] example [dot] edu\nMail\nFuzzing\nCompilers — LLVM"
        );
        assert!(!page.text.contains("fake@spam.com") && !page.text.contains("color"));
    }
}
