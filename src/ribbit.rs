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

mod parsers {
    use std::collections::HashMap;

    use anyhow::bail;
    use nom::{
        branch::alt,
        bytes::complete::{is_not, tag},
        character::complete::{digit1, newline},
        combinator::{eof, map_res},
        multi::fold_many0,
        sequence::{delimited, terminated, tuple},
        IResult,
    };

    use super::Summary;
    use super::SummaryEntry;

    fn seqn(s: &str) -> IResult<&str, u32> {
        map_res(digit1, |s: &str| s.parse::<u32>())(s)
    }

    pub(crate) fn summary(s: &str) -> IResult<&str, Summary> {
        delimited(
            tag("Product!STRING:0|Seqn!DEC:4|Flags!STRING:0\n"),
            map_res::<_, _, _, _, anyhow::Error, _, _>(
                tuple((
                    delimited(tag("## seqn = "), seqn, newline),
                    fold_many0(
                        terminated(
                            tuple((
                                is_not("|"),
                                delimited(tag("|"), seqn, tag("|")),
                                alt((tag("bgdl"), tag("cdn"), tag(""))),
                            )),
                            newline,
                        ),
                        || Ok(HashMap::new()),
                        |m, (s, n, t)| {
                            let mut m = m?;
                            let mut v: SummaryEntry = m.remove(s).unwrap_or_default();
                            match t {
                                "" => v.seqn = Some(n),
                                "cdn" => v.cdn = Some(n),
                                "bgdl" => v.bgdl = Some(n),
                                _ => bail!("internal error"),
                            }
                            m.insert(s.to_owned(), v);
                            Ok(m)
                        },
                    ),
                )),
                |(seqn, entries)| {
                    Ok(Summary {
                        seqn,
                        entries: entries?,
                    })
                },
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
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use velcro::hash_map as m;

    #[test]
    fn it_works() -> Result<()> {
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
}
