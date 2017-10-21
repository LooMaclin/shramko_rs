#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate serde_json;
extern crate tokio_core;
extern crate tokio_io;
extern crate hyper;
extern crate futures_await as futures;
#[macro_use]
extern crate error_chain;
extern crate futures_cpupool;

mod errors;
mod image;

use hyper::Client;
use tokio_core::reactor::Core;
use serde_json::Value;
use std::io::Write;
use std::fs::File;
use futures::prelude::*;
use futures::future::join_all;
use std::sync::Arc;
use hyper::client::HttpConnector;
use hyper::Uri;
use errors::{Error, ErrorKind, ResultExt};
use image::Image;
use futures_cpupool::CpuPool;
use futures_cpupool::CpuFuture;

const POOL_SIZE: usize = 64;

#[async]
fn get_and_deser(client: Arc<Client<HttpConnector>>, url: Uri) -> Result<Value, Error> {
    let body = await!(client.get(url).and_then(|r| r.body().concat2())).chain_err(|| "Cannot resolve response body")?;
    let json = serde_json::from_slice(&body).chain_err(|| "Cannot deserialize response from slice")?;
    Ok(json)
}

#[async]
fn download_chunk(client: Arc<Client<HttpConnector>>, url: Uri) -> Result<hyper::Chunk, Error> {
    let body = await!(client.get(url).and_then(|r| r.body().concat2())).chain_err(|| "Cannot resolve response body")?;
    Ok(body)
}

#[async]
fn resolve_image_info(client: Arc<Client<HttpConnector>>, name: &'static str) -> Result<Image, Error> {
    let url = format!("http://lurkmore.to/api.php?action=query&titles={}&prop=imageinfo&format=json&iiprop=timestamp|user|url", &name)
        .parse::<hyper::Uri>().chain_err(|| "Cannot parse api url")?;
    let info = await!(get_and_deser(client, url))?;
    let pages = info["query"]["pages"].as_object().chain_err(|| "/query/pages in response body not object")?;
    for image in pages.values() {
        let url = image["imageinfo"][0]["url"].as_str().chain_err(|| "url field in imageinfo[0] is not str")?;
        let title = image["title"].as_str().chain_err(|| "title field in image is not str")?;
        return Ok(Image {
            title: String::from(title),
            url: String::from(url)
        })
    }
    Err(ErrorKind::Msg("Image in values not found".to_owned()).into())
}

#[async]
fn download_image(client: Arc<Client<HttpConnector>>, image: Image) -> Result<hyper::Chunk, Error>  {
    let url = image.url.parse::<hyper::Uri>().unwrap();
    let body = await!(download_chunk(client, url))?;
    Ok(body)
}

#[async]
fn save_image(pool: CpuPool, path: String, body: hyper::Chunk) -> Result<(), Error> {
    let future: CpuFuture<(), Error> = pool.spawn_fn(move|| {
        let mut file = File::create(&path).chain_err(|| "Error when create file for downloaded image")?;
        file.write(&body).chain_err(|| "Error when writing downloaded image to file")?;
        Ok(())
    });
    await!(future)
}

#[async]
fn download_and_save_image(pool: CpuPool, client: Arc<Client<HttpConnector>>, image: Image) -> Result<String, Error> {
    let path = format!("./{}", image.title);
    let body = await!(download_image(client, image))?;
    await!(save_image(pool, path.clone(), body))?;
    Ok(path)
}

#[async]
fn main_future(pool: CpuPool, client: Arc<Client<HttpConnector>>, names: Vec<&'static str>) -> Result<Vec<String>, Error> {
    let future = names.into_iter().map(|name| resolve_image_info(client.clone(), name)).collect::<Vec<_>>();
    let future : Vec<Image> = await!(join_all(future))?;
    let future = future.into_iter().map(| image | download_and_save_image(pool.clone(),client.clone(), image)).collect::<Vec<_>>();
    let future = await!(join_all(future))?;
    Ok(future)
}

fn main () {
    let mut core = Core::new().expect("Core creating error");
    let pool = CpuPool::new(POOL_SIZE);
    let handle = core.handle();
    let client = Arc::new(Client::new(&handle));
    let names = vec!["Файл:Postervesch.jpg", "Файл:World of Drugs.jpg"];
    let paths = core.run(main_future(pool, Arc::clone(&client), names)).expect("Error in event loop");
    for path in paths {
        println!("Created file: {}", &path);
    }
}