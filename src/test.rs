#![allow(dead_code)]

use anyhow::{anyhow, Result};
use regex::Regex;
use std::io::{BufRead, BufReader, Write};
use serde::Deserialize;

use crate::minecraft_client::{ConnectionReadHalf, ConnectionWriteHalf};

pub fn run_tests(reader: ConnectionReadHalf, mut writer: ConnectionWriteHalf) -> Result<()> {
    writeln!(writer, "/gamerule sendCommandFeedback false")?;
    
    writeln!(writer, "/function mctest:plan")?;    
    let mut reader = BufReader::new(reader);
    let Plan { length } = reader.read_plan()?;
    println!("1..{length}");

    let mut test_commands = Vec::new();
    writeln!(writer, "/function mctest:list")?;
    for _ in 0..length {
        let test_command = reader.read_plaintext()?;
        test_commands.push(test_command);
    }

    for command in test_commands {
        writeln!(writer, "{command}")?;
        let response = reader.read_plaintext()?;
        println!("{response}");
    }

    Ok(())
}

#[derive(Debug)]
struct Plan {
    length: usize,
}

enum DirectiveType {
    Todo,
    Skip,
    Unknown(String)
}

struct Directive {
    directive_type: DirectiveType,
    reason: Option<String>
}

struct TestPoint {
    ok: bool,
    // number: Option<usize>,
    // description: String,
    // directive: Directive,
    // yaml: Yaml
}

struct Subtest {

}

enum Element {}

trait BufReadExt {
    fn read_plaintext(&mut self) -> Result<String>;
    fn read_plan(&mut self) -> Result<Plan>;
}

impl<R: BufRead> BufReadExt for R {
    fn read_plaintext(&mut self) -> Result<String> {
        let mut buf = String::new();
        self.read_line(&mut buf)?;
        let text_component: TextComponent = serde_json::from_str(&mut buf)?;
        Ok(component_to_plaintext(text_component))
    }

    fn read_plan(&mut self) -> Result<Plan> {
        let text = self.read_plaintext()?;

        let length = Regex::new(r"1..(?<length>\d+)")?
            .captures(&text)
            .ok_or(anyhow!("Failed to parse `{text}` as a Plan"))?
            .name("length")
            .ok_or(anyhow!("Malformed regex"))?
            .as_str()
            .parse()?;

        Ok(Plan { length })
    }
}

fn json_to_plaintext(json: &str) -> Result<String> {
    Ok(component_to_plaintext(serde_json::from_str(json)?))
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum TextComponent {
    Text {
        text: String,
        #[serde(default = "Vec::new")]
        extra: Vec<TextComponent>,
    },
    Translate {
        translate: String,
        #[serde(default = "Vec::new")]
        extra: Vec<TextComponent>,
    }
}

impl TextComponent {
    fn text(&self) -> String {
        match self {
            TextComponent::Text { text, .. } => text.clone(),
            TextComponent::Translate { translate, .. } => translate.clone(),
        }
    }

    fn extra(&self) -> Vec<TextComponent> {
        match self {
            TextComponent::Text { extra, .. } => extra.clone(),
            TextComponent::Translate { extra, .. } => extra.clone(),
        }
    }

    fn is_text(&self) -> bool {
        match self {
            TextComponent::Text { .. } => true,
            _ => false,
        }
    }
}

fn component_to_plaintext(text_component: TextComponent) -> String {
    let mut text = text_component.text();
    let extra = text_component.extra();
    for tc in extra {
        text.push_str(&component_to_plaintext(tc));
    }
    text
}