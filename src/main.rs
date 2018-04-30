#![allow(unused_imports)]
#![allow(dead_code)]

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
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;


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
}


struct WebProxyService {
    server_conf: ServerConf,
    proxies: Vec<reqwest::Proxy>,
}

impl WebProxyService {
    fn handle_route(&self, req: Request, route: &RouteConf) -> Response {
        // text resource application/xml
        // binary resource application/x-tar
        let mut path = req.path().to_string()[route.location.len()..].to_string();
        if let Some(query) = req.query() {
            path.push_str("?");
            path.push_str(query);
        }

        let url = format!("{}{}", route.proxy_pass, path);

        let mut headers: Headers = Headers::new();
        let cached_path = if let Some(ref root_cache_path) = self.server_conf.cached {
            let url = Url::from_str(route.proxy_pass.as_str()).unwrap();
            let cached_path = PathBuf::from_iter([root_cache_path.as_str(),  format!("{}@{}", url.host().unwrap(), url.port().unwrap_or(80)).as_str(), req.path().trim_left_matches("/")].iter());
            if cached_path.exists() && cached_path.is_file() {
                let last_modify: DateTime<Utc> = DateTime::from(fs::metadata(cached_path.as_path()).unwrap().modified().unwrap());
                headers.set_raw("If-Modified-Since", last_modify.format("%a, %d %b %Y %H:%M:%S GMT").to_string());
            }
            Some(cached_path)
        } else {
            None
        };

        let mut client_builder = reqwest::Client::builder();

        for p in &self.proxies {
            client_builder.proxy(p.clone());
        }
        match client_builder.build().unwrap().get(url.as_str()).headers(headers).send() {
            Ok(mut res) => {
                println!("proxy_pass:Path: {}", res.url().as_str());
                println!("proxy_pass:Status: {}", res.status());
                println!("proxy_pass:Headers:\n{}", res.headers());
                match res.status() {
                    StatusCode::NotModified => {
                        println!("read from cache");
                        let cached_path = cached_path.unwrap();
                        let display = cached_path.display().to_string();
                        let mut file = match File::open(cached_path.as_path()) {
                            Err(why) => panic!("couldn't open {}: {}", display, why.description()),
                            Ok(file) => file,
                        };
                        let mut data = Vec::new();
                        file.read_to_end(&mut data).unwrap();
                        Response::new()
                            .with_headers(res.headers().clone())
                            .with_body(data)
                    }
                    StatusCode::Ok => {
                        println!("read from http body");
                        let mut data = Vec::new();
                        res.copy_to(&mut data).unwrap();

                        // replace url
                        if let Some(content_type) = res.headers().get_raw("Content-Type") {
                            let content_type = String::from_utf8(content_type.one().unwrap().to_vec()).unwrap();
                            match content_type.as_str() {
                                "application/json" | "" => {
                                    let mut tmp = String::from_utf8(data).unwrap();
                                    if let Some(ref replaces) = route.text_replace {
                                        for i in replaces {
                                            tmp = tmp.replace(i[0].as_str(), i[1].as_str());
                                        }
                                    }
                                    let host = req.headers().get::<Host>().unwrap();
                                    let from = route.proxy_pass.as_str();
                                    let to = format!("http://{}:{}/{}", host.hostname(), host.port().unwrap_or(80),route.location);
                                    tmp = tmp.replace(from, to.as_str());
                                    let url_regex: &Regex = &URL_REGEX;
                                    // replace all url
                                    if let Some(base_url) = &self.server_conf.replace_base_url {
                                        tmp = url_regex.replace_all(tmp.as_str(), |caps: &Captures|format!("{}/@?proxy={}", base_url, encode(&caps["url"]))).to_string();
                                    }

                                    data = tmp.into_bytes();
                                },
                                _ => ()
                            }
                        };

                        if let Some(ref cached_path) = cached_path {
                            // Last-Modified
                            // If-Modified-Since
                            if let Some(t) = res.headers().get_raw("Last-Modified") {
                                let last_modified = String::from_utf8(t.one().unwrap().to_vec()).unwrap();
                                let last_modified = HttpDate::from_str(last_modified.as_str()).ok().and_then(|x|Some(SystemTime::from(x))).unwrap();

                                write_to_file(cached_path.as_path(), last_modified, data.as_ref());
                            }
                        };
                        Response::new()
                            .with_headers(res.headers().clone()).with_header(ContentLength(data.len() as u64))
                            .with_body(data)
                    },
                    StatusCode::NotFound =>Response::new().with_status(StatusCode::NotFound),
                    _ => panic!("Unknown error")
                }
            }
            Err(e) => {
                let msg = e.to_string();
                Response::new()
                    .with_header(ContentLength(msg.len() as u64))
                    .with_body(msg)
            }
        }

    }
    fn handle(&self, req: Request) -> Response {
        if req.path() == "/@" && req.query().is_some() {
            let query_str = req.query().unwrap().to_string();
            let queries: Vec<&str> = query_str.split('&').collect();
            if queries.len() == 1 && queries[0].starts_with("proxy=") {
                let dst = String::from_utf8(decode(&queries[0][6..]).unwrap()).unwrap();
                let route = RouteConf{
                    location: "/@".to_string(),
                    proxy_pass: dst,
                    text_replace: None,
                };
                println!("{:?}", route);
                return self.handle_route(req, &route);
            }
        }

        for route in &self.server_conf.routes {
            if !(req.path().starts_with(route.location.as_str())) {
                continue;
            };
            return self.handle_route(req, route);
        }

        // proxy others
        let msg = format!("others ");
        Response::new()
            .with_header(ContentLength(msg.len() as u64))
            .with_body(msg)
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
        Box::new(futures::future::ok(self.handle(req)))
    }
}

fn write_to_file(file: &Path, time_stamp: SystemTime, data: &[u8]) {
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

#[derive(Debug, Deserialize, Serialize)]
struct RootConf {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    servers: Vec<ServerConf>,
    cached: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ServerConf {
    listen: String,
    server_name: Option<String>,
    routes: Vec<RouteConf>,
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    cached: Option<String>,
    replace_base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RouteConf {
    location: String,
    proxy_pass: String,
    text_replace: Option<Vec<[String;2]>>,
}

fn load_config() -> RootConf {
    let path = Path::new("web-proxy");
    let display = path.display();
    let mut file = match File::open(&path) {
        Err(why) => panic!("couldn't open {}: {}", display, why.description()),
        Ok(file) => file,
    };
    let mut s = String::new();
    match file.read_to_string(&mut s) {
        Err(why) => panic!("couldn't read {}: {}", display, why.description()),
        Ok(_) => (),
    };
    toml::from_str(s.as_str()).ok().and_then(|mut root_conf: RootConf| {
        for i in 0..root_conf.servers.len() {
            if root_conf.servers[i].https_proxy.is_none() {
                root_conf.servers[i].https_proxy =  root_conf.https_proxy.clone();
            }
            if root_conf.servers[i].http_proxy.is_none() {
                root_conf.servers[i].http_proxy =  root_conf.http_proxy.clone();
            }
        }
        Some(root_conf)
    }).expect("parse config error.")
}

fn main() {

    let conf = load_config();
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





