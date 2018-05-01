//#![allow(unused_imports)]
//#![allow(dead_code)]

extern crate reqwest;
extern crate hyper;
extern crate futures;
extern crate chrono;
extern crate byteorder;
extern crate toml;
extern crate serde;
extern crate filetime;
extern crate url;
extern crate regex;
extern crate base64;
extern crate mime;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;

mod mime_types;
mod conf;

use conf::{ServerConf, RouteConf};


use reqwest::RedirectPolicy;
use futures::future::Future;
use base64::{encode, decode};
use hyper::{StatusCode};
use url::Url;
use hyper::server::{Http, Request, Response, Service};
use hyper::header::{HttpDate, Headers, ContentLength, Host};
use chrono::prelude::*;
use regex::{Regex, Captures};

use std::error::Error;
use std::str::FromStr;
use std::fs;
use std::fs::{File};
use std::path::{Path, PathBuf};
use std::io::{Write, Read};
use std::convert::From;
use std::time::SystemTime;
use std::iter::FromIterator;

lazy_static! {
static ref URL_REGEX: Regex = Regex::new(r#"(?P<url>(?P<schema>[Hh][Tt]{2}[Pp][Ss]?)://(?P<host>((?P<domain>[a-zA-Z0-9\-\.]+[a-zA-Z]{1,10})|(?P<ip>((25[0-5]|2[0-4]\d|((1\d{2})|([1-9]?\d)))\.){3}(25[0-5]|2[0-4]\d|((1\d{2})|([1-9]?\d))))))(:(?P<port>6[0-5]{2}[0-3][0-5]|[1-5]\d{4}|[1-9]\d{3}|[1-9]\d{2}|[1-9]\d|[0-9]))?(?P<path>/[^\s"\\]*|/?))"#).unwrap();

static ref  A_ABSOLUTE_HREF: Regex = Regex::new(r#"(?P<a><a\s+(?:[^>]*?\s+)?href=["'](?P<absolute_path>/(.*?))["']>.+</a>)"#).unwrap();

static ref  IMG_ABSOLUTE_HREF: Regex = Regex::new(r#"(?P<img><img\s+(?:[^>]*?\s+)?src=["'](?P<absolute_path>/(.*?))["']>)"#).unwrap();
}

const BASE_PROXY: &'static str = "/@/proxy";

struct WebProxyService {
    server_conf: ServerConf,
    proxies: Vec<reqwest::Proxy>,
}

impl WebProxyService {
    fn cache_file(&self, file: &Path, time_stamp: SystemTime, data: &[u8]) {
        let parent = file.parent().unwrap();
        if !(parent.exists()) {
            let _ = fs::create_dir_all(parent).unwrap();
        }
        {
            let display = file.display();
            if file.exists() {
                fs::remove_file(file).expect("delete file failed.");
            }
            File::create(&file)
                .unwrap_or_else(|why|panic!("couldn't create {}: {}", display, why.description()))
                .write_all(data).unwrap_or_else(|why|panic!("couldn't write to {}: {}", display, why.description()));
        }
        let time_stamp = filetime::FileTime::from_system_time(time_stamp);
        filetime::set_file_times(file, time_stamp, time_stamp).expect("set modified time failed.");
    }

    fn read_from_cache(&self, cached_path: PathBuf, mut headers: Headers) -> Response {
        println!("read from cache");
        let display = cached_path.display().to_string();
        let mut file = match File::open(cached_path.as_path()) {
            Err(why) => panic!("couldn't open {}: {}", display, why.description()),
            Ok(file) => file,
        };
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        let mime = cached_path.extension().and_then(|x|Some(mime_types::get_mime_type(x.to_str().unwrap_or(".")))).unwrap_or(mime::APPLICATION_OCTET_STREAM);
        headers.set_raw("Content-Type", mime.to_string());
        headers.set_raw("Connection", "close");
        headers.set(ContentLength(data.len() as u64));
        headers.remove_raw("Set-Cookie");
        Response::new().with_headers(headers).with_body(data)
    }

    fn encode_url(&self, base_url: &str, src_url:&str, query: Option<&str>) -> String {
        let mut url = if src_url.ends_with('/') {
            format!("{}{}/{}/", base_url, BASE_PROXY, encode(src_url.trim_right_matches('/')))
        } else {
            format!("{}{}/{}", base_url, BASE_PROXY, encode(src_url))
        };

        if let Some(query) = query {
            url.push_str("?");
            url.push_str(query);
        }
        url
    }

    fn decode_url(&self, url: &str) -> String {
        let start = BASE_PROXY.len() + 1;
        let url = &url[start..];

        if let Some(i) = url.find('/') {
            format!("{}/{}", String::from_utf8(decode(&url[..i]).unwrap()).unwrap(), &url[i+1 ..])
        } else {
            format!("{}", String::from_utf8(decode(url).unwrap()).unwrap())
        }
    }

    fn replace_url(&self, req: &Request, route: &RouteConf, data: String) -> String {
        let host = req.headers().get::<Host>().unwrap();

        let get_without_schema = |u: &Url| format!("{}{}{}", u.host().and_then(|x| Some(x.to_string())).unwrap_or("".to_string()), u.port().and_then(|x| Some(x.to_string())).unwrap_or("".to_string()) ,u.path());

        let url = Url::from_str(route.proxy_pass.as_str()).unwrap();

        let mut from = get_without_schema(&url);;

        let to = if let Some(port) = host.port() {
            format!("{}:{}{}", host.hostname(), port, route.location)
        } else {
            format!("{}{}", host.hostname(), route.location)
        };
        if !to.ends_with('/') && from.ends_with('/') {
            let index = from.len() - 1;
            from.remove(index);
        }

        URL_REGEX.replace(data.as_str(), |x: &Captures| {
            let all = x.name("url").unwrap();
            let url1 = Url::from_str(all.as_str()).unwrap();
            if get_without_schema(&url1).starts_with(from.as_str())  {
                // currently support http only.
                return  all.as_str().replace(format!("{}://{}", url1.scheme(),from).as_str(), format!("http://{}", to).as_str());
            }

            if let Some(base_url) = self.server_conf.replace_base_url.as_ref() {
                self.encode_url(base_url, all.as_str(), req.query())
            } else {
                all.as_str().to_string()
            }
        }).to_string()
    }

    fn replace_text(&self, req: Request, res: &reqwest::Response, route: &RouteConf, data: Vec<u8>) -> Vec<u8> {
        if let Some(content_type) = res.headers().get_raw("Content-Type") {
            let content_type = String::from_utf8(content_type.one().unwrap().to_vec()).unwrap();
            if let Some(ref mime_types) = self.server_conf.url_replace_mime {
                if mime_types.iter().any(|x| content_type.starts_with(x)) {
                    let mut tmp = String::from_utf8(data).unwrap();
                    if let Some(ref replaces) = route.text_replace {
                        for i in replaces {
                            tmp = tmp.replace(i[0].as_str(), i[1].as_str());
                        }
                    }

                    tmp = self.replace_url(&req, route, tmp);

                    // replace all url
                    println!("replace <a> absolute path");
                    tmp = A_ABSOLUTE_HREF.replace_all(tmp.as_str(), |x: &Captures| {
                        let all = x.name("a").unwrap();
                        let mut replace = all.as_str().to_string();
                        replace.insert_str(x.name("absolute_path").unwrap().start() - all.start() ,req.path().trim_right_matches('/'));
                        replace
                    }).to_string();

                    println!("replace <img> absolute path");
                    tmp = IMG_ABSOLUTE_HREF.replace_all(tmp.as_str(), |x: &Captures| {
                        let all = x.name("img").unwrap();
                        let mut replace = all.as_str().to_string();
                        replace.insert_str(x.name("absolute_path").unwrap().start() - all.start() ,req.path().trim_right_matches('/'));
                        replace
                    }).to_string();

                    return  tmp.into_bytes();
                }
            }
        }
        data
    }

    fn send_message(&self, msg: String) -> Response {
        let mut headers = Headers::new();
        headers.set_raw("Content-Type ", "text/plain; charset=utf8");
//                headers.set(ContentLength(msg.len() as u64));
        Response::new().with_headers(headers).with_body(msg)
    }

    fn handle_route(&self, req: Request, route: &RouteConf) -> Response {
        // text resource application/xml
        // binary resource application/x-tar
        let mut path = if req.path().starts_with(BASE_PROXY) {
            String::new()
        } else {
            req.path().to_string()[route.location.len()..].to_string()
        };

        if let Some(query) = req.query() {
            path.push_str("?");
            path.push_str(query);
        }

        let url = format!("{}{}", route.proxy_pass, path);

        let mut headers: Headers = Headers::new();
        let mut cached_path = if let Some(ref root_cache_path) = self.server_conf.cached {
            let url = Url::from_str(route.proxy_pass.as_str()).unwrap();
            let mut cached_path = PathBuf::from_iter([root_cache_path.as_str(),  format!("{}@{}", url.host().unwrap(), url.port().unwrap_or(80)).as_str(), req.path().trim_left_matches("/")].iter());
            if cached_path.exists(){
                if cached_path.is_dir() {
                    for index in route.index.as_ref().unwrap_or(&default_files()) {
                        cached_path.push(index);
                        if cached_path.is_file() {
                            break;
                        }
                        cached_path.pop();
                    }
                }
                if cached_path.is_file() {
                    let last_modify: DateTime<Utc> = DateTime::from(fs::metadata(cached_path.as_path()).unwrap().modified().unwrap());
                    headers.set_raw("If-Modified-Since", last_modify.format("%a, %d %b %Y %H:%M:%S GMT").to_string());
                }
            }
            Some(cached_path)
        } else {
            None
        };

        let mut client_builder = reqwest::Client::builder();

        for p in &self.proxies {
            client_builder.proxy(p.clone());
        }
        match client_builder.redirect(RedirectPolicy::none()).build().unwrap().get(url.as_str()).headers(headers).send() {
            Ok(mut res) => {
                println!("proxy_pass:Path: {}", res.url().as_str());
                println!("proxy_pass:Status: {}", res.status());
                println!("proxy_pass:Headers:\n{}", res.headers());
                match res.status() {
                    StatusCode::NotModified => self.read_from_cache(cached_path.unwrap(), res.headers().clone()),
                    StatusCode::MovedPermanently | StatusCode::Found=> {
                        if let Some(location) = res.headers().get_raw("Location") {
                            let location = self.replace_url(&req, route, String::from_utf8(location.one().unwrap().to_vec()).unwrap());
                            println!("redirect to : {}", location);
                            let mut headers: Headers = Headers::new();
                            headers.set_raw("Location", location);
                            Response::new().with_headers(headers).with_status(res.status())
                        } else {
                            self.send_message("redirect".to_string())
                        }
                    },
                    StatusCode::Ok => {
                        println!("read from http body");
                        let mut data = Vec::new();
                        res.copy_to(&mut data).unwrap();

                        // replace url
                        data = self.replace_text(req,&res, route, data);

                        if let Some(ref mut cached_path) = cached_path {
                            // Last-Modified
                            // If-Modified-Since
                            if let Some(t) = res.headers().get_raw("Last-Modified") {
                                let last_modified = String::from_utf8(t.one().unwrap().to_vec()).unwrap();
                                let last_modified = HttpDate::from_str(last_modified.as_str()).ok().and_then(|x|Some(SystemTime::from(x))).unwrap();

                                if let Some(content_type) = res.headers().get_raw("Content-Type") {
                                    let content_type = String::from_utf8(content_type.one().unwrap().to_vec()).unwrap();
                                    if content_type.contains("text/html") && !url.ends_with(".html") && !url.ends_with(".htm") {
                                        cached_path.push("index.html");
                                    }
                                }

                                self.cache_file(cached_path.as_path(), last_modified, data.as_ref());
                            }
                        };
                        let mut headers: Headers = res.headers().clone();
//                        headers.set(ContentLength(data.len() as u64)); // Transfer-Encoding
                        headers.remove_raw("Transfer-Encoding");
                        Response::new().with_headers(headers).with_body(data)
                    },
                    StatusCode::NotFound =>Response::new().with_status(StatusCode::NotFound),
                    _ => panic!("Unknown error{}", res.status())
                }
            }
            Err(e) => {
                // try read from cache
                if let Some(cached_path) = cached_path {
                    if cached_path.exists() &&cached_path.is_file(){
                        return self.read_from_cache(cached_path, Headers::new());
                    }

                }
                self.send_message(e.to_string())
            }
        }

    }

    fn handle(&self, req: Request) -> Response {
        if req.path().starts_with(BASE_PROXY) {
            let dst = self.decode_url(req.path());
            let route = RouteConf{
                location: BASE_PROXY.to_string(),
                proxy_pass: dst,
                text_replace: None,
                index: None
            };
            println!("{:?}", route);
            return self.handle_route(req, &route);
        }

        for route in &self.server_conf.routes {
            if !(req.path().starts_with(route.location.as_str())) {
                continue;
            };
            return self.handle_route(req, route);
        }

        // proxy others
        self.send_message(format!("others "))
    }
}

impl Service for WebProxyService {
    // boilerplate hooking up hyper's server types
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    // The future representing the eventual Response your call will
    // resolve to. This can change to whatever Future you need.
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        println!("origin path:{}", req.path());
        println!("origin headers:{}", req.headers());
        Box::new(futures::future::ok(self.handle(req)))
    }
}


fn default_files() -> Vec<String> {
    vec!["index.html".to_string()]
}

fn main() {
    let conf = conf::load_config();
    if conf.servers.len() != 1 {
        println!("currently just support one server.");
        return;
    }
    let mut server_conf: ServerConf = conf.servers.get(0).unwrap().clone();

    server_conf.routes.sort_by(|x, y| Ord::cmp( &y.location.len(), &x.location.len()));
    let address = server_conf.listen.parse().unwrap();

    let server = Http::new().bind(&address, move || {
        let server_conf = server_conf.clone();
        let mut proxies = Vec::new();

        if let Some(p) =server_conf.http_proxy.as_ref().and_then(|x| reqwest::Proxy::http(x).ok()) {
            proxies.push(p);
        }

        if let Some(p) =  server_conf.https_proxy.as_ref().and_then(|x| reqwest::Proxy::https(x).ok()) {
            proxies.push(p);
        }
        Ok(WebProxyService {
            server_conf,
            proxies
        })
    }).unwrap();
    println!("listening on {}", address);
    server.run().unwrap();
}





