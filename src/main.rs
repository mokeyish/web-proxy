#![allow(dead_code)]
#![allow(unused_imports)]

extern crate reqwest;
extern crate hyper;
extern crate futures;
extern crate chrono;
extern crate byteorder;
extern crate toml;
extern crate serde;
#[macro_use]
extern crate serde_derive;

use futures::future::Future;

use hyper::StatusCode;
use hyper::server::{Http, Request, Response, Service};
use hyper::header::{HttpDate, Headers, ContentLength};

use std::error::Error;
use std::str::FromStr;
use std::fs;
use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Write, Read};
use std::convert::From;
use std::time::SystemTime;
use std::iter::FromIterator;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, FixedOffset, TimeZone, Utc, NaiveDateTime};


struct WebProxyService {
    server_conf: ServerConf,

    http_proxy: Option<reqwest::Proxy>,
    https_proxy: Option<reqwest::Proxy>,
}

impl WebProxyService {
    fn handle(&self, req: Request) -> Response {
        println!("请求路径:{}", req.path());
        println!("请求头:{}", req.headers());

        for route in &self.server_conf.routes {
            let location = format!("/{}", route.location);
            if !(req.path().starts_with(location.as_str())) {
                continue;
            };
            return  {
                // 文本 application/xml
                // 文件资源 application/x-tar
                let path = req.path().to_string()[location.len()..].to_string();

                let url = format!("{}{}", route.proxy_pass, path);

                let mut headers: Headers = Headers::new();
                let mut cached_file = None;
                let cached_path = if let Some(ref root_cache_path) = route.cached {
                    let cached_path = PathBuf::from_iter([root_cache_path.as_str(), path.as_str().trim_left_matches("/")].iter());
                    println!("根缓存路径:{}", root_cache_path);
                    println!("相对缓存路径:{}", path);
                    println!("缓存路径:{:?}", cached_path);
                    if cached_path.exists() && cached_path.is_file() {
                        let display = cached_path.display().to_string();
                        let mut file = match File::open(cached_path.as_path()) {
                            // `io::Error` 的 `description` 方法返回一个描述错误的字符串。
                            Err(why) => panic!("couldn't open {}: {}", display, why.description()),
                            Ok(file) => file,
                        };
                        let time_stamp = file.read_i64::<LittleEndian>().unwrap_or_else(|e| {
                            println!("读取时间戳失败:{}", e.description());
                            Utc::now().timestamp()
                        });
                        let last_modify = NaiveDateTime::from_timestamp(time_stamp, 0);
                        cached_file = Some(file);
                        println!("If-Modified-Since");
                        headers.set_raw("If-Modified-Since", last_modify.format("%a, %d %b %Y %H:%M:%S GMT").to_string());
                    }
                    Some(cached_path)
                } else {
                    None
                };

                let mut client_builder = reqwest::Client::builder();

                if let Some(ref p) = self.http_proxy {
                    client_builder.proxy(p.clone());
                }
                if let Some(ref p) = self.https_proxy {
                    client_builder.proxy(p.clone());
                }
                println!("访问上级");
                match client_builder.build().unwrap().get(url.as_str()).headers(headers).send() {
                    Ok(mut res) => {
                        println!("代理:Path: {}", res.url().as_str());
                        println!("代理:Status: {}", res.status());
                        println!("代理:Headers:\n{}", res.headers());
                        match res.status() {
                            StatusCode::NotModified => {
                                // 读取缓存
                                println!("读取缓存");

                                let mut data = Vec::new();
                                cached_file.unwrap().read_to_end(&mut data).unwrap();
                                println!("从缓存读取文件");
                                Response::new()
                                    .with_headers(res.headers().clone())
                                    .with_body(data)
                            }
                            StatusCode::Ok => {
                                println!("读取数据");
                                // 读取数据
                                let mut data = Vec::new();
                                res.copy_to(&mut data).unwrap();
                                if let Some(ref _cached) = route.cached {
                                    // Last-Modified
                                    // If-Modified-Since
                                    if let Some(t) = res.headers().get_raw("Last-Modified") {
                                        let last_modified = String::from_utf8(t.one().unwrap().to_vec()).unwrap();
                                        let m = parse_to_date_time(last_modified.as_str()).unwrap();
                                        if let Some(cached_path) = cached_path {
                                            write_to_file(cached_path.as_path(), m.timestamp(), data.as_ref());
                                        }
                                        println!("上次更改时间是:{:?}", m);
                                    }
                                }
                                Response::new()
                                    .with_headers(res.headers().clone())
                                    .with_body(data)
                            }
                            _ => panic!("未知的错误")
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
        }

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
        // We're currently ignoring the Request
        // And returning an 'ok' Future, which means it's ready
        // immediately, and build a Response with the 'PHRASE' body.
        Box::new(futures::future::ok(self.handle(req)))
    }
}

//quick_main!(run);

fn write_to_file(file: &Path, time_stamp: i64, data: &[u8]) {
    let parent = file.parent().unwrap();
    if !(parent.exists()) {
        let _ = fs::create_dir_all(parent).unwrap();
    }
    let display = file.display();

    let mut file = match File::create(&file) {
        Err(why) => panic!("couldn't create {}: {}", display, why.description()),
        Ok(file) => file,
    };

    file.write_i64::<LittleEndian>(time_stamp).expect("write time stamp failed.");
    match file.write_all(data) {
        Err(why) => panic!("couldn't write to {}: {}", display, why.description()),
        Ok(_) => println!("successfully cached {}", display),
    }
}

fn parse_to_date_time(data_str: &str) -> Result<DateTime<Utc>, hyper::Error> {
    Ok(DateTime::<Utc>::from(SystemTime::from(HttpDate::from_str(data_str)?)))
}

#[derive(Debug, Deserialize, Serialize)]
struct RootConf {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    servers: Vec<ServerConf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ServerConf {
    listen: String,
    server_name: Option<String>,
    routes: Vec<RouteConf>,
    http_proxy: Option<String>,
    https_proxy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RouteConf {
    location: String,
    proxy_pass: String,
    cached: Option<String>,
}

fn load_config() -> Option<RootConf> {
    // 给所需的文件创建一个路径
    let path = Path::new("web-proxy");
    let display = path.display();

    // 以只读方式打开路径，返回 `io::Result<File>`
    let mut file = match File::open(&path) {
        // `io::Error` 的 `description` 方法返回一个描述错误的字符串。
        Err(why) => panic!("couldn't open {}: {}", display, why.description()),
        Ok(file) => file,
    };

    // 读取文件内容到一个字符串，返回 `io::Result<usize>`
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
    })
}

fn main() {
    if let Some(conf) = load_config() {
        if conf.servers.len() != 1 {
            println!("currently just support one server.");
            return;
        }
        let mut server_conf: ServerConf = conf.servers.get(0).unwrap().clone();
        server_conf.routes.sort_by(|x, y| Ord::cmp( &y.location.len(), &x.location.len()));
        let address = server_conf.listen.parse().unwrap();

        let server = Http::new().bind(&address, move || {
            let server_conf = server_conf.clone();
            let http_proxy = server_conf.http_proxy.as_ref().and_then(|x| reqwest::Proxy::http(x).ok());
            let https_proxy = server_conf.https_proxy.as_ref().and_then(|x| reqwest::Proxy::https(x).ok());
            Ok(WebProxyService {
                server_conf,
                http_proxy,
                https_proxy
            })
        }).unwrap();
        println!("listening on {}", address);
        server.run().unwrap();
    }
}





