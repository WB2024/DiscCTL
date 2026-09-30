//! A small interpreter for MusicBrainz Picard's file naming scripts.
//!
//! Picard users already have a script that says exactly how their library is laid out. Rather
//! than approximate it with a template, this runs the script itself: paste it in, and every
//! path comes out the way Picard would have made it. It covers the language as used in real
//! naming scripts: `%variables%`, `$functions(...)` with lazy `$if`/`$if2`, `\` escapes, and
//! Picard's rule that line breaks and the indentation after them are ignored.
//!
//! Supported functions: noop set get unset if if2 and or not eq ne gt gte lt lte add sub mul div
//! mod len left right substr num upper lower title trim replace rreplace firstalphachar
//! truncate in startswith endswith swapprefix. Anything else is reported as an error rather
//! than silently producing a wrong path.

use std::collections::HashMap;

pub const DEFAULT_SCRIPT: &str = include_str!("default_script.txt");

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Var(String),
    Call(String, Vec<Vec<Node>>),
}

struct Parser<'a> {
    src: Vec<char>,
    pos: usize,
    _s: std::marker::PhantomData<&'a ()>,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser { src: s.chars().collect(), pos: 0, _s: std::marker::PhantomData }
    }

    fn peek(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    /// Parse until the end of input (`nested` false) or the `,` / `)` that ends an argument.
    fn sequence(&mut self, nested: bool) -> Result<Vec<Node>, String> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, nodes: &mut Vec<Node>| {
            if !text.is_empty() {
                nodes.push(Node::Text(std::mem::take(text)));
            }
        };
        while let Some(c) = self.peek() {
            match c {
                ',' | ')' if nested => break,
                // Line breaks, and the indentation after them, are not part of the script's text.
                '\n' | '\r' => {
                    self.pos += 1;
                    while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                        self.pos += 1;
                    }
                }
                '\\' => {
                    self.pos += 1;
                    if let Some(n) = self.peek() {
                        text.push(n);
                        self.pos += 1;
                    }
                }
                '%' => {
                    // A variable, if there is a closing % before anything that can't be a name.
                    let rest = &self.src[self.pos + 1..];
                    match rest.iter().position(|ch| *ch == '%') {
                        Some(end) if end > 0 && rest[..end].iter().all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | ':' | '-')) => {
                            flush(&mut text, &mut nodes);
                            nodes.push(Node::Var(rest[..end].iter().collect()));
                            self.pos += end + 2;
                        }
                        _ => {
                            text.push('%');
                            self.pos += 1;
                        }
                    }
                }
                '$' => {
                    let rest = &self.src[self.pos + 1..];
                    let name_len = rest.iter().take_while(|ch| ch.is_alphanumeric() || **ch == '_').count();
                    if name_len > 0 && rest.get(name_len) == Some(&'(') {
                        flush(&mut text, &mut nodes);
                        let name: String = rest[..name_len].iter().collect();
                        self.pos += 1 + name_len + 1;
                        let mut args: Vec<Vec<Node>> = Vec::new();
                        loop {
                            args.push(self.sequence(true)?);
                            match self.peek() {
                                Some(',') => self.pos += 1,
                                Some(')') => {
                                    self.pos += 1;
                                    break;
                                }
                                _ => return Err(format!("${name}( is missing its closing )")),
                            }
                        }
                        nodes.push(Node::Call(name, args));
                    } else {
                        text.push('$');
                        self.pos += 1;
                    }
                }
                _ => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        flush(&mut text, &mut nodes);
        Ok(nodes)
    }
}

struct Interp<'a> {
    meta: &'a HashMap<String, String>,
    vars: HashMap<String, String>,
    steps: usize,
}

fn truthy(s: &str) -> bool {
    !s.is_empty()
}

fn boolean(b: bool) -> String {
    if b { "1".into() } else { String::new() }
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

fn int(s: &str) -> i64 {
    s.trim().parse::<i64>().unwrap_or(0)
}

impl Interp<'_> {
    fn eval(&mut self, nodes: &[Node]) -> Result<String, String> {
        let mut out = String::new();
        for n in nodes {
            self.steps += 1;
            if self.steps > 200_000 {
                return Err("The script ran for too long".into());
            }
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Var(name) => out.push_str(&self.var(name)),
                Node::Call(name, args) => out.push_str(&self.call(name, args)?),
            }
        }
        Ok(out)
    }

    fn var(&self, name: &str) -> String {
        if let Some(v) = self.vars.get(name) {
            return v.clone();
        }
        // Tag values can't add folders to the path.
        self.meta.get(name).map(|v| v.replace(['/', '\\'], "_")).unwrap_or_default()
    }

    fn call(&mut self, name: &str, args: &[Vec<Node>]) -> Result<String, String> {
        // Functions that decide for themselves which arguments to evaluate.
        match name {
            "noop" => return Ok(String::new()),
            "if" => {
                if args.len() < 2 || args.len() > 3 {
                    return Err("$if needs 2 or 3 arguments".into());
                }
                let c = self.eval(&args[0])?;
                return if truthy(&c) { self.eval(&args[1]) } else if args.len() == 3 { self.eval(&args[2]) } else { Ok(String::new()) };
            }
            "if2" => {
                for a in args {
                    let v = self.eval(a)?;
                    if truthy(&v) {
                        return Ok(v);
                    }
                }
                return Ok(String::new());
            }
            "and" => {
                for a in args {
                    if !truthy(&self.eval(a)?) {
                        return Ok(String::new());
                    }
                }
                return Ok("1".into());
            }
            "or" => {
                for a in args {
                    if truthy(&self.eval(a)?) {
                        return Ok("1".into());
                    }
                }
                return Ok(String::new());
            }
            _ => {}
        }

        let mut v: Vec<String> = Vec::with_capacity(args.len());
        for a in args {
            v.push(self.eval(a)?);
        }
        let arg = |i: usize| v.get(i).map(String::as_str).unwrap_or("");
        let need = |n: usize| if v.len() < n { Err(format!("${name} needs at least {n} argument(s)")) } else { Ok(()) };

        Ok(match name {
            "set" => {
                need(2)?;
                self.vars.insert(v[0].trim().to_string(), v[1].clone());
                String::new()
            }
            "get" => {
                need(1)?;
                self.var(v[0].trim())
            }
            "unset" => {
                need(1)?;
                self.vars.remove(v[0].trim());
                String::new()
            }
            "not" => boolean(!truthy(arg(0))),
            "eq" => { need(2)?; boolean(arg(0) == arg(1)) }
            "ne" => { need(2)?; boolean(arg(0) != arg(1)) }
            "gt" | "gte" | "lt" | "lte" => {
                need(2)?;
                let ord = match (num(arg(0)), num(arg(1))) {
                    (Some(a), Some(b)) => a.partial_cmp(&b),
                    _ => Some(arg(0).cmp(arg(1))),
                };
                let o = ord.unwrap_or(std::cmp::Ordering::Equal);
                use std::cmp::Ordering::*;
                boolean(match name {
                    "gt" => o == Greater,
                    "gte" => o != Less,
                    "lt" => o == Less,
                    _ => o != Greater,
                })
            }
            "add" => { need(2)?; v.iter().map(|x| int(x)).sum::<i64>().to_string() }
            "sub" => { need(2)?; (int(arg(0)) - v[1..].iter().map(|x| int(x)).sum::<i64>()).to_string() }
            "mul" => { need(2)?; v.iter().map(|x| int(x)).product::<i64>().to_string() }
            "div" => { need(2)?; if int(arg(1)) == 0 { String::new() } else { (int(arg(0)) / int(arg(1))).to_string() } }
            "mod" => { need(2)?; if int(arg(1)) == 0 { String::new() } else { (int(arg(0)) % int(arg(1))).to_string() } }
            "len" => { need(1)?; arg(0).chars().count().to_string() }
            "left" => { need(2)?; arg(0).chars().take(int(arg(1)).max(0) as usize).collect() }
            "right" => {
                need(2)?;
                let chars: Vec<char> = arg(0).chars().collect();
                let n = (int(arg(1)).max(0) as usize).min(chars.len());
                chars[chars.len() - n..].iter().collect()
            }
            "substr" => {
                need(3)?;
                let (from, to) = (int(arg(1)).max(0) as usize, int(arg(2)).max(0) as usize);
                arg(0).chars().skip(from).take(to.saturating_sub(from)).collect()
            }
            "truncate" => { need(2)?; arg(0).chars().take(int(arg(1)).max(0) as usize).collect() }
            "num" => {
                need(2)?;
                match arg(0).trim().parse::<i64>() {
                    Ok(n) => format!("{:0width$}", n, width = int(arg(1)).max(0) as usize),
                    Err(_) => String::new(),
                }
            }
            "upper" => { need(1)?; arg(0).to_uppercase() }
            "lower" => { need(1)?; arg(0).to_lowercase() }
            "title" => {
                need(1)?;
                let mut out = String::new();
                let mut start = true;
                for c in arg(0).chars() {
                    if start && c.is_alphabetic() {
                        out.extend(c.to_uppercase());
                        start = false;
                    } else {
                        out.extend(c.to_lowercase());
                        start = !c.is_alphanumeric() && c != '\'';
                    }
                }
                out
            }
            "trim" => {
                need(1)?;
                match v.get(1).and_then(|c| c.chars().next()) {
                    Some(c) => arg(0).trim_matches(c).to_string(),
                    None => arg(0).trim().to_string(),
                }
            }
            "replace" => { need(3)?; if arg(1).is_empty() { arg(0).to_string() } else { arg(0).replace(arg(1), arg(2)) } }
            "rreplace" => {
                need(3)?;
                let re = regex::Regex::new(arg(1)).map_err(|e| format!("$rreplace has a bad pattern ({}): {e}", arg(1)))?;
                re.replace_all(arg(0), arg(2)).to_string()
            }
            "firstalphachar" => {
                need(1)?;
                let other = if v.len() > 1 { arg(1) } else { "#" };
                match arg(0).chars().next() {
                    Some(c) if c.is_alphabetic() => c.to_string(),
                    _ => other.to_string(),
                }
            }
            "in" => { need(2)?; boolean(arg(0).contains(arg(1))) }
            "startswith" => { need(2)?; boolean(arg(0).starts_with(arg(1))) }
            "endswith" => { need(2)?; boolean(arg(0).ends_with(arg(1))) }
            "swapprefix" => {
                need(1)?;
                let text = arg(0);
                let prefixes: Vec<&str> = if v.len() > 1 { v[1..].iter().map(String::as_str).collect() } else { vec!["A", "The"] };
                let mut out = text.to_string();
                for p in prefixes {
                    if let Some(rest) = text.strip_prefix(&format!("{p} ")) {
                        out = format!("{rest}, {p}");
                        break;
                    }
                }
                out
            }
            other => return Err(format!("The script uses ${other}(), which RustyDisc doesn't support yet")),
        })
    }
}

/// Check a script for syntax errors without running it.
pub fn check(script: &str) -> Result<(), String> {
    Parser::new(script).sequence(false).map(|_| ())
}

/// Run `script` for one track. `meta` holds the track's tags under Picard's variable names.
pub fn run(script: &str, meta: &HashMap<String, String>) -> Result<String, String> {
    let ast = Parser::new(script).sequence(false)?;
    let mut i = Interp { meta, vars: HashMap::new(), steps: 0 };
    i.eval(&ast)
}

/// Turn the script's output into safe path components: no empty parts, no `.` or `..`, no
/// stray spaces, no control characters. Names are otherwise left exactly as the script made them.
pub fn to_components(output: &str) -> Vec<String> {
    output
        .split('/')
        .map(|p| p.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string())
        .filter(|p| !p.is_empty() && p != "." && p != "..")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn path(pairs: &[(&str, &str)]) -> String {
        to_components(&run(DEFAULT_SCRIPT, &meta(pairs)).unwrap()).join("/")
    }

    #[test]
    fn the_default_script_parses() {
        check(DEFAULT_SCRIPT).unwrap();
    }

    #[test]
    fn a_normal_album() {
        let p = path(&[
            ("albumartist", "The Troggs"), ("albumartistsort", "Troggs, The"), ("artist", "The Troggs"), ("album", "Wild Thing"),
            ("date", "1966-06-01"), ("tracknumber", "1"), ("totaltracks", "12"), ("title", "Wild Thing"), ("totaldiscs", "1"),
        ]);
        assert_eq!(p, "T/Troggs, The/[1966] Wild Thing/01 - The Troggs. Wild Thing");
    }

    #[test]
    fn original_date_wins_and_track_numbers_pad_to_the_album_size() {
        let p = path(&[
            ("albumartist", "Band"), ("album", "Box"), ("date", "2011-01-01"), ("originaldate", "1975-05-05"),
            ("tracknumber", "7"), ("totaltracks", "120"), ("title", "Seven"), ("artist", "Band"),
        ]);
        assert_eq!(p, "B/Band/[1975] Box/007 - Band. Seven");
    }

    #[test]
    fn multi_disc_albums_get_a_disc_folder_with_its_subtitle() {
        let base = [("albumartist", "Band"), ("album", "Live"), ("date", "2001"), ("tracknumber", "3"), ("totaltracks", "10"), ("title", "Song"), ("artist", "Band"), ("totaldiscs", "2"), ("discnumber", "2")];
        assert_eq!(path(&base), "B/Band/[2001] Live/CD 2/03 - Band. Song");
        let mut with = base.to_vec();
        with.push(("discsubtitle", "Encore"));
        assert_eq!(path(&with), "B/Band/[2001] Live/CD 2 - Encore/03 - Band. Song");
    }

    #[test]
    fn various_artists_have_no_artist_folder() {
        let p = path(&[
            ("albumartist", "Various Artists"), ("musicbrainz_albumartistid", "89ad4ac3-39f7-470e-963a-56509c546377"),
            ("album", "Now 1"), ("date", "1983"), ("artist", "Some Band"), ("tracknumber", "2"), ("totaltracks", "20"), ("title", "Hit"),
        ]);
        assert_eq!(p, "Various Artists/[1983] Now 1/02 - Some Band. Hit");
    }

    #[test]
    fn the_release_comment_is_added_in_brackets() {
        let p = path(&[("albumartist", "Band"), ("album", "Rec"), ("date", "1999"), ("_releasecomment", "remastered"), ("tracknumber", "1"), ("totaltracks", "5"), ("title", "A"), ("artist", "Band")]);
        assert_eq!(p, "B/Band/[1999] Rec (remastered)/01 - Band. A");
    }

    #[test]
    fn unsafe_characters_are_replaced_and_tags_cannot_add_folders() {
        let p = path(&[
            ("albumartist", "AC/DC"), ("albumartistsort", "AC/DC"), ("artist", "AC/DC"), ("album", "Who Made Who: Live?"),
            ("date", "1986"), ("tracknumber", "1"), ("totaltracks", "9"), ("title", "For Those About To Rock (We Salute You)"),
        ]);
        assert_eq!(p, "A/AC_DC/[1986] Who Made Who_ Live_/01 - AC_DC. For Those About To Rock (We Salute You)");
    }

    #[test]
    fn missing_tags_fall_back_like_the_script_says() {
        assert_eq!(path(&[]), "#/[Unknown Artist]/[0000] [Unknown Album]/01 - [Unknown Artist]. [Unknown Title]");
    }

    #[test]
    fn a_long_file_name_is_cut_to_the_limit() {
        let long = "x".repeat(300);
        let out = run(DEFAULT_SCRIPT, &meta(&[("albumartist", "B"), ("album", "A"), ("title", &long), ("artist", "B"), ("tracknumber", "1"), ("totaltracks", "2"), ("date", "2000")])).unwrap();
        let file = out.rsplit('/').next().unwrap();
        assert_eq!(file.chars().count(), 200);
        assert!(file.ends_with("..."));
    }

    #[test]
    fn language_basics() {
        let m = meta(&[("a", "1"), ("empty", "")]);
        let r = |s: &str| run(s, &m).unwrap();
        assert_eq!(r("$if(%a%,yes,no)"), "yes");
        assert_eq!(r("$if(%empty%,yes,no)"), "no");
        assert_eq!(r("$if(%empty%,yes)"), "");
        assert_eq!(r("$if2(%empty%,%a%,z)"), "1");
        assert_eq!(r("$set(x, spaced)[%x%]"), "[ spaced]"); // spaces inside an argument count
        assert_eq!(r("$set(x,1)\n   $set(y,2)\n%x%%y%"), "12"); // line breaks and indentation don't
        assert_eq!(r("\\$notacall \\%x\\% \\(ok\\)"), "$notacall %x% (ok)");
        assert_eq!(r("$num(7,3)|$left(abcdef,3)|$right(abcdef,2)|$upper(ab)|$len(héé)"), "007|abc|ef|AB|3");
        assert_eq!(r("$gt(10,9)|$gt(a,b)|$lt(2,10)|$eq(a,a)|$ne(a,a)|$not(x)"), "1||1|1||");
        assert_eq!(r("$add(1,2)|$sub(10,3)|$mul(2,3)|$div(7,2)|$mod(7,2)"), "3|7|6|3|1");
        assert_eq!(r("$replace(a-b-c,-,+)|$rreplace(a1b22,[0-9]+,#)|$firstalphachar(9lives,#)|$firstalphachar(Édith,#)"), "a+b+c|a#b#|#|É");
        assert_eq!(r("$swapprefix(The Who)|$title(hello wORLD)|$trim( x )|$substr(abcdef,1,3)"), "Who, The|Hello World|x|bc");
        assert_eq!(r("100% sure $ done"), "100% sure $ done");
        assert!(run("$nosuchfunction(x)", &m).unwrap_err().contains("doesn't support"));
        assert!(run("$if(a,b", &m).is_err());
        assert!(run("$rreplace(a,[,x)", &m).unwrap_err().contains("bad pattern"));
    }

    #[test]
    fn output_is_split_into_safe_components() {
        assert_eq!(to_components("A/ B /../C//./D\n"), ["A", "B", "C", "D"]);
    }
}
