// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::{thread, time};

use crate::hglib::{init, Basic, Client, HG};

pub struct TestClient {
    pub path: PathBuf,
    pub client: Client,
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.client.close().unwrap();
        assert!(fs::remove_dir_all(&self.path).is_ok());
    }
}

impl TestClient {
    pub fn new(name: &str, configs: &[&str]) -> Self {
        env::set_var("HGUSER", "test");

        let tmp = env::temp_dir();
        let path = tmp.join(name);
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir(&path).unwrap();
        let _ = env::set_current_dir(&path);

        let path_str = path.to_str().unwrap();

        assert!(HG!(init, dest = path_str).is_ok());

        while File::create(".hg/hgrc").is_err() {
            let ten_millis = time::Duration::from_millis(10);
            thread::sleep(ten_millis);
        }

        let client = Client::open(path_str, "UTF-8", configs).unwrap();

        Self { path, client }
    }

    #[allow(dead_code)]
    pub fn append(&self, path: &str, lines: &[&str]) {
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.path.join(path))
            .unwrap();
        let mut writer = BufWriter::new(file);
        let _ = write!(&mut writer, "{}", lines.join("\n"));
    }

    #[allow(dead_code)]
    pub fn get_path(&self, path: &str) -> String {
        let path = self.path.join(path);
        path.to_str().unwrap().to_string()
    }

    #[allow(dead_code)]
    pub fn reopen(&mut self) {
        self.client.close().unwrap();
        self.client = Client::open(self.path.to_str().unwrap(), "UTF-8", &[]).unwrap();
    }
}
