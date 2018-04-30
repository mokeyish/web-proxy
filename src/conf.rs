use std::path::Path;
use std::fs::File;
use toml;
use std::error::Error;
use std::io::Read;

#[derive(Debug, Deserialize, Serialize)]
pub struct RootConf {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub servers: Vec<ServerConf>,
    pub cached: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConf {
    pub listen: String,
    pub server_name: Option<String>,
    pub routes: Vec<RouteConf>,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub cached: Option<String>,
    pub replace_base_url: Option<String>,
    pub url_replace_mime: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouteConf {
    pub location: String,
    pub proxy_pass: String,
    pub index: Option<Vec<String>>,
    pub text_replace: Option<Vec<[String;2]>>,
}


pub fn load_config() -> RootConf {
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