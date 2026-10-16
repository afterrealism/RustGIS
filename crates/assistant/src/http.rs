//! Sending requests on the desktop and the command line (blocking: call it off the UI thread).

use crate::provider::{HttpRequest, HttpResponse, allowed_url, reply_limit};

/// Sends `req` and reads the response, whatever its status.
pub fn send(req: &HttpRequest) -> Result<HttpResponse, String> {
    send_timeout(req, std::time::Duration::from_secs(600))
}

/// Sends with an overall deadline, including reading the response body.
pub fn send_timeout(req: &HttpRequest, timeout: std::time::Duration) -> Result<HttpResponse, String> {
    // The address checked is the one used.
    let url = req.url.trim();
    if !allowed_url(url) {
        return Err(format!("{url}: only HTTPS addresses, or plain HTTP on this computer or network, are allowed"));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Models can think for minutes on a hard request.
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        // An API key is never sent on to another address.
        .max_redirects(0)
        .build()
        .into();
    let ua = concat!("RustGIS/", env!("CARGO_PKG_VERSION"));
    let result = match (req.method, &req.body) {
        ("POST", body) => {
            let mut b = agent.post(url).header("User-Agent", ua);
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            b.send(body.as_deref().unwrap_or(""))
        }
        _ => {
            let mut b = agent.get(url).header("User-Agent", ua);
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            b.call()
        }
    };
    let mut resp = result.map_err(|e| format!("couldn't reach {}: {e}", host(url)))?;
    let status = resp.status().as_u16();
    let request_id = resp.headers().get("x-request-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = resp.body_mut().with_config().limit(reply_limit(req) as u64).read_to_vec().map_err(|e| format!("reading the reply: {e}"))?;
    Ok(HttpResponse { status, body, request_id })
}

/// The scheme and host of a URL (errors never repeat a path that might carry a key).
fn host(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Never the user name or password.
    format!("{scheme}://{}", authority.rsplit('@').next().unwrap_or(""))
}
