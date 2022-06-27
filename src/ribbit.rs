use std::collections::HashMap;

use anyhow::Result;

#[derive(Debug, Default, PartialEq)]
pub struct SummaryEntry {
    seqn: Option<u32>,
    cdn: Option<u32>,
    bgdl: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub struct Summary {
    seqn: u32,
    entries: HashMap<String, SummaryEntry>,
}

#[derive(Debug, PartialEq)]
pub struct VersionsEntry {
    region: String,
    build_config: u128,
    cdn_config: u128,
    key_config: Option<u128>,
    build_id: u32,
    name: String,
    product_config: u128,
}

#[derive(Debug, PartialEq)]
pub struct Versions {
    seqn: u32,
    entries: HashMap<String, VersionsEntry>,
}

#[derive(Debug, PartialEq)]
pub struct CDNsEntry {
    region: String,
    path: String,
    hosts: Vec<String>,
    servers: Vec<String>,
    config_path: String,
}

#[derive(Debug, PartialEq)]
pub struct CDNs {
    seqn: u32,
    entries: HashMap<String, CDNsEntry>,
}

mod parsers {
    use std::collections::HashMap;

    use nom::{
        branch::alt,
        bytes::complete::{is_not, tag, take_until},
        character::complete::{digit1, hex_digit1, newline},
        combinator::{eof, map, map_res, opt},
        multi::{fold_many0, separated_list0},
        sequence::{delimited, terminated, tuple},
        IResult,
    };

    use super::{CDNs, Summary, Versions};
    use super::{CDNsEntry, SummaryEntry, VersionsEntry};

    fn dec32(s: &str) -> IResult<&str, u32> {
        map_res(digit1, |s: &str| s.parse::<u32>())(s)
    }

    fn hex128(s: &str) -> IResult<&str, u128> {
        map_res(hex_digit1, |s: &str| u128::from_str_radix(s, 16))(s)
    }

    pub(crate) fn strs(s: &str) -> IResult<&str, Vec<String>> {
        separated_list0(tag(" "), map(is_not(" |"), |s: &str| s.to_owned()))(s)
    }

    pub(crate) fn summary(s: &str) -> IResult<&str, Summary> {
        delimited(
            tag("Product!STRING:0|Seqn!DEC:4|Flags!STRING:0\n"),
            map(
                tuple((
                    delimited(tag("## seqn = "), dec32, newline),
                    fold_many0(
                        terminated(
                            tuple((
                                is_not("|"),
                                delimited(tag("|"), dec32, tag("|")),
                                alt((tag("bgdl"), tag("cdn"), tag(""))),
                            )),
                            newline,
                        ),
                        HashMap::new,
                        |mut m, (s, n, t)| {
                            let mut v: SummaryEntry = m.remove(s).unwrap_or_default();
                            match t {
                                "" => v.seqn = Some(n),
                                "cdn" => v.cdn = Some(n),
                                "bgdl" => v.bgdl = Some(n),
                                _ => panic!("internal error"),
                            }
                            m.insert(s.to_owned(), v);
                            m
                        },
                    ),
                )),
                |(seqn, entries)| Summary { seqn, entries },
            ),
            eof,
        )(s)
    }

    pub(crate) fn versions(s: &str) -> IResult<&str, Versions> {
        delimited(
            tuple((take_until("\n"), newline)),
            map(
                tuple((
                    delimited(tag("## seqn = "), dec32, newline),
                    fold_many0(
                        map(
                            tuple((
                                terminated(is_not("|"), tag("|")),
                                terminated(hex128, tag("|")),
                                terminated(hex128, tag("|")),
                                terminated(opt(hex128), tag("|")),
                                terminated(dec32, tag("|")),
                                terminated(is_not("|"), tag("|")),
                                terminated(hex128, newline),
                            )),
                            |(r, bcfg, c, k, bid, v, p)| VersionsEntry {
                                region: r.to_owned(),
                                build_config: bcfg,
                                cdn_config: c,
                                key_config: k,
                                build_id: bid,
                                name: v.to_owned(),
                                product_config: p,
                            },
                        ),
                        HashMap::new,
                        |mut m, e| {
                            m.insert(e.region.clone(), e);
                            m
                        },
                    ),
                )),
                |(seqn, entries)| Versions { seqn, entries },
            ),
            eof,
        )(s)
    }

    pub(crate) fn cdns(s: &str) -> IResult<&str, CDNs> {
        delimited(
            tuple((take_until("\n"), newline)),
            map(
                tuple((
                    delimited(tag("## seqn = "), dec32, newline),
                    fold_many0(
                        map(
                            tuple((
                                terminated(is_not("|"), tag("|")),
                                terminated(is_not("|"), tag("|")),
                                terminated(strs, tag("|")),
                                terminated(strs, tag("|")),
                                terminated(is_not("\n"), newline),
                            )),
                            |(a, b, c, d, e)| CDNsEntry {
                                region: a.to_owned(),
                                path: b.to_owned(),
                                hosts: c,
                                servers: d,
                                config_path: e.to_owned(),
                            },
                        ),
                        HashMap::new,
                        |mut m, e| {
                            m.insert(e.region.clone(), e);
                            m
                        },
                    ),
                )),
                |(seqn, entries)| CDNs { seqn, entries },
            ),
            eof,
        )(s)
    }
}

pub struct Ribbit {
    stream: std::net::TcpStream,
}

impl Ribbit {
    pub fn new() -> Result<Ribbit> {
        Ok(Ribbit {
            stream: std::net::TcpStream::connect("us.version.battle.net:1119")?,
        })
    }
    fn command<T>(&mut self, cmd: &[u8], parser: fn(&str) -> nom::IResult<&str, T>) -> Result<T> {
        use anyhow::{bail, ensure, Context};
        use mime_multipart::{read_multipart, Node, Part};
        use std::io::Write;

        self.stream.write_all(cmd)?;
        self.stream.write_all(b"\r\n")?;
        self.stream.flush()?;
        let nodes = read_multipart(&mut self.stream, false).context("mime")?;
        ensure!(nodes.len() == 2);
        match &nodes[0] {
            Node::Part(Part { body, .. }) => {
                let (_, v) = parser(std::str::from_utf8(body)?).map_err(|e| e.to_owned())?;
                Ok(v)
            }
            _ => bail!("mime"),
        }
    }
    pub fn summary(&mut self) -> Result<Summary> {
        self.command(b"v1/summary", parsers::summary)
    }
    pub fn versions(&mut self, product: &str) -> Result<Versions> {
        self.command(
            format!("v1/products/{}/versions", product).as_bytes(),
            parsers::versions,
        )
    }
    pub fn cdns(&mut self, product: &str) -> Result<CDNs> {
        self.command(
            format!("v1/products/{}/cdns", product).as_bytes(),
            parsers::cdns,
        )
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use velcro::hash_map as m;
    use velcro::vec as v;

    #[test]
    fn summary() -> Result<()> {
        let input = concat!(
            "Product!STRING:0|Seqn!DEC:4|Flags!STRING:0\n",
            "## seqn = 42\n",
            "moo|123|\n",
            "moo|456|cdn\n",
            "cow|789|\n",
        );
        let expected = super::Summary {
            seqn: 42,
            entries: m! {
                "moo".to_string(): super::SummaryEntry {
                    seqn: Some(123),
                    cdn: Some(456),
                    bgdl: None,
                },
                "cow".to_string(): super::SummaryEntry {
                    seqn: Some(789),
                    cdn: None,
                    bgdl: None,
                },
            },
        };
        assert_eq!(expected, super::parsers::summary(input)?.1);
        Ok(())
    }

    #[test]
    fn strs() {
        assert_eq!(
            Ok(("", v!["a".to_string(), "b".to_string(), "c".to_string()])),
            super::parsers::strs("a b c")
        );
        assert_eq!(
            Ok(("|de", v!["a".to_string(), "b".to_string(), "c".to_string()])),
            super::parsers::strs("a b c|de")
        );
    }

    #[test]
    fn cdns() {
        let input = concat!(
            "FooBarBaz\n",
            "## seqn = 42\n",
            "us|a/b|foo.com bar.com|http://foo.com/?baz http://bar.com/?quux|c/d/e\n",
            "eu|v/w|bar.com foo.com|http://bar.com/?quux http://foo.com/?baz|x/y/z\n",
        );
        let expected = super::CDNs {
            seqn: 42,
            entries: m! {
                "us".to_string(): super::CDNsEntry {
                    region: "us".to_string(),
                    path: "a/b".to_string(),
                    hosts: v!["foo.com".to_string(), "bar.com".to_string()],
                    servers: v!["http://foo.com/?baz".to_string(), "http://bar.com/?quux".to_string()],
                    config_path: "c/d/e".to_string(),
                },
                "eu".to_string(): super::CDNsEntry {
                    region: "eu".to_string(),
                    path: "v/w".to_string(),
                    hosts: v!["bar.com".to_string(), "foo.com".to_string()],
                    servers: v!["http://bar.com/?quux".to_string(), "http://foo.com/?baz".to_string()],
                    config_path: "x/y/z".to_string(),
                },
            },
        };
        assert_eq!(Ok(("", expected)), super::parsers::cdns(input));
    }
}
