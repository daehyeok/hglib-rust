// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.

extern crate byteorder;
extern crate subprocess;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::result::Result;
use subprocess::{Popen, PopenConfig, Redirection};

pub trait Runner {
    /// Run a command
    fn runcommand(&mut self, args: &[&str]) -> Result<(Vec<u8>, i32), HglibError>;
}

#[derive(Debug)]
pub struct Client {
    /// the server process
    server: Popen,
    /// the encoding used for this process
    encoding: String,
}

pub struct Basic {}

#[derive(Debug)]
pub struct HglibError {
    pub code: i32,
    pub out: Option<Vec<u8>>,
    msg: String,
}

impl HglibError {
    pub(crate) fn handle_err(x: Result<(Vec<u8>, i32), HglibError>) -> Result<bool, HglibError> {
        match x {
            Ok((_, code)) => Ok(code == 0),
            Err(err) => {
                if err.code == 0 {
                    Ok(true)
                } else if err.code == 1 {
                    Ok(false)
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl<T: std::string::ToString> From<T> for HglibError {
    fn from(err: T) -> HglibError {
        HglibError {
            code: -1,
            out: None,
            msg: err.to_string(),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close().unwrap();
    }
}

impl Client {
    /// Open a new hglib client
    /// # Example
    /// ```
    /// extern crate hglib;
    ///
    /// use hglib::{hg, Client, log::self};
    ///
    /// fn main() {
    ///     let mut client = Client::open("/home/calixte/dev/mozilla/mozilla-central.hg", "UTF-8", &[]).unwrap();
    ///     let res = client.log(log::Arg {
    ///         revrange: &["tip", "-2"],
    ///        ..Default::default()
    ///     }).unwrap();
    ///     eprintln!("{:?}", res);
    ///
    ///     let res = hg!(client, log, revrange = &["tip:-2"]).unwrap();
    ///     eprintln!("{:?}", res);
    /// }
    /// ```
    pub fn open(path: &str, encoding: &str, configs: &[&str]) -> Result<Client, HglibError> {
        let mut env: Vec<(OsString, OsString)> = env::vars_os().collect();
        env.push((OsString::from("HGPLAIN"), OsString::from("1")));
        if !encoding.is_empty() {
            env.push((OsString::from("HGENCODING"), OsString::from(encoding)));
        }
        let mut args = vec!["hg", "serve", "--cmdserver", "pipe", "-R", path];
        for c in configs.iter() {
            args.push("--config");
            args.push(c);
        }
        let mut server = Popen::create(
            &args,
            PopenConfig {
                stdout: Redirection::Pipe,
                stdin: Redirection::Pipe,
                stderr: Redirection::Pipe,
                env: Some(env),
                cwd: Some(OsString::from(path)),
                ..Default::default()
            },
        )?;
        let encoding = Client::read_hello(&mut server)?;
        let client = Client { server, encoding };
        Ok(client)
    }

    /// Close the client
    pub fn close(&mut self) -> Result<(), HglibError> {
        self.server.terminate()?;
        self.server.wait()?;
        Ok(())
    }

    fn read_hello(server: &mut Popen) -> Result<String, HglibError> {
        let stdout = server.stdout.as_mut().unwrap();
        let mut chan: Vec<u8> = vec![0; 1];
        let n = stdout.read(&mut chan)?;
        if n != 1 || chan[0] != b'o' {
            return Err("Cannot read hello".into());
        }

        let len = stdout.read_u32::<BigEndian>()? as usize;
        let mut data: Vec<u8> = vec![0; len];

        let n = stdout.read(&mut data)?;
        if n != len {
            return Err("Cannot read hello (invalid length)".into());
        }

        let out = std::str::from_utf8(&data)?;
        let out: Vec<&str> = out.split('\n').collect();

        if !out[0].contains("capabilities: ") {
            return Err("Cannot read hello: no capabilities ".into());
        }

        if !out[1].contains("encoding: ") {
            return Err("Cannot read hello: no encoding ".into());
        }

        Ok(out[1]["encoding: ".len()..].to_string())
    }

    fn read_data(
        mut to_read: usize,
        output: &mut Vec<u8>,
        stdout: &mut File,
    ) -> Result<(), HglibError> {
        let mut pos = output.len();
        output.resize(pos + to_read, 0);
        loop {
            let n = stdout.read(&mut output[pos..])?;
            if n == to_read {
                break;
            }
            to_read -= n;
            pos += n;
        }
        Ok(())
    }
}

impl Runner for Client {
    fn runcommand(&mut self, args: &[&str]) -> Result<(Vec<u8>, i32), HglibError> {
        /* Write the data on stdin:
        runcommand\n
        len(arg0\0arg1\0arg2...)
        arg0\0arg1\0arg2... */
        let mut stdin = self.server.stdin.as_mut().unwrap();
        let args_size: usize = args.iter().map(|arg| -> usize { arg.len() }).sum();
        let size = args_size + args.len() - 1;
        writeln!(&mut stdin, "runcommand")?;
        stdin.write_u32::<BigEndian>(size as u32)?;
        if let Some((first, args)) = args.split_first() {
            write!(&mut stdin, "{}", first)?;
            for arg in args {
                write!(&mut stdin, "\0{}", arg)?;
            }
        }
        stdin.flush()?;

        /* Read the data on stdout:
        o{u32 = len}{data}
        ...
        r{u32} */
        let stdout = self.server.stdout.as_mut().unwrap();
        let mut out = Vec::<u8>::with_capacity(4096);
        let mut chan: Vec<u8> = vec![0; 1];
        let mut returned_err: Option<String> = None;
        loop {
            let n = stdout.read(&mut chan)?;
            if n != 1 {
                return Err("Empty stdout".into());
            }
            let len = stdout.read_u32::<BigEndian>()? as usize;
            match chan[0] {
                b'e' => {
                    // We've an error
                    let mut err = Vec::<u8>::with_capacity(512);
                    Client::read_data(len, &mut err, stdout)?;
                    let err = String::from_utf8(err)?;
                    returned_err = Some(err);
                }
                b'o' => {
                    Client::read_data(len, &mut out, stdout)?;
                }
                b'r' => {
                    let mut code: Vec<u8> = vec![0; len];
                    stdout.read_exact(&mut code)?;
                    let mut cur = Cursor::new(&code);
                    let code = cur.read_i32::<BigEndian>()?;
                    return if let Some(msg) = returned_err {
                        Err(HglibError {
                            code,
                            out: Some(out),
                            msg,
                        })
                    } else if code != 0 {
                        Err(HglibError {
                            code,
                            out: Some(out),
                            msg: "".to_string(),
                        })
                    } else {
                        Ok((out, code))
                    };
                }
                _ => {
                    return Err(format!("Hglib error: invalid channel {}", chan[0] as char).into());
                }
            }
        }
    }
}

impl Runner for Basic {
    fn runcommand(&mut self, args: &[&str]) -> Result<(Vec<u8>, i32), HglibError> {
        let env: Vec<(OsString, OsString)> = env::vars_os().collect();
        let mut command = Vec::with_capacity(args.len() + 1);
        command.push("hg");
        command.extend(args);

        let mut process = Popen::create(
            &command,
            PopenConfig {
                stdout: Redirection::Pipe,
                cwd: Some(OsString::from(std::env::current_dir().unwrap())),
                env: Some(env),
                ..Default::default()
            },
        )?;

        process.wait()?;

        Ok((Vec::new(), 0))
    }
}
