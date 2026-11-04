//! Read configuration as data. Compiler commands are never executed.
use super::super::relations::{Binding, BindingTarget, FileFacts, Lookup};
use super::{Language, ModuleRoot};
use crate::{EngineError, EngineResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
const MAX_CONFIGURATION_BYTES: u64 = 8 << 20;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationInput {
    pub path: String,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CompileSettings {
    quote: Vec<PathBuf>,
    include: Vec<PathBuf>,
    system: Vec<PathBuf>,
    after: Vec<PathBuf>,
    macros: Vec<String>,
    macro_values: Vec<String>,
    language: Option<Language>,
    issue: Option<String>,
}
#[derive(Debug)]
pub(super) struct Configuration {
    workspace: PathBuf,
    pub inputs: Vec<ConfigurationInput>,
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub prefixes: BTreeMap<PathBuf, Vec<String>>,
    compile: BTreeMap<String, CompileSettings>,
    roots: Vec<ModuleRoot>,
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
#[derive(Deserialize)]
struct CompileCommand {
    directory: String,
    file: String,
    arguments: Option<Vec<String>>,
    command: Option<String>,
}
fn failure(message: &str) -> EngineError {
    EngineError::invalid_argument(format!("graph source configuration: {message}"))
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
fn contents(workspace: &Path, path: &str) -> EngineResult<Option<Vec<u8>>> {
    let path = workspace.join(path);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(EngineError::from_io("read graph source configuration", &e)),
    };
    let canonical = path
        .canonicalize()
        .map_err(|e| EngineError::from_io("resolve graph source configuration", &e))?;
    if !canonical.starts_with(workspace) {
        return Err(failure("configuration is outside the workspace"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIGURATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| EngineError::from_io("read graph source configuration", &e))?;
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err(failure("configuration exceeds byte limit"));
    }
    Ok(Some(bytes))
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
pub(crate) fn validate_inputs(workspace: &Path, inputs: &[ConfigurationInput]) -> EngineResult<()> {
    if !inputs_match(workspace, inputs)? {
        return Err(EngineError::resource_busy(
            "graph source configuration changed; rebuild the graph",
        ));
    }
    Ok(())
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
pub(crate) fn inputs_match(workspace: &Path, inputs: &[ConfigurationInput]) -> EngineResult<bool> {
    let workspace = workspace
        .canonicalize()
        .map_err(|e| EngineError::from_io("resolve configuration workspace", &e))?;
    for input in inputs {
        if input.path.is_empty()
            || input.path.contains(['\\', ':', '\0'])
            || Path::new(&input.path).is_absolute()
            || Path::new(&input.path)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(failure("invalid configuration input path"));
        }
        if contents(&workspace, &input.path)?
            .as_ref()
            .map(|b| crate::utils::sha256_hex(b))
            != input.digest
        {
            return Ok(false);
        }
    }
    Ok(true)
}
impl Configuration {
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    pub fn read(workspace: &Path, roots: &[ModuleRoot]) -> EngineResult<Self> {
        let workspace = workspace
            .canonicalize()
            .map_err(|e| EngineError::from_io("resolve configuration workspace", &e))?;
        let mut result = Self {
            workspace,
            inputs: vec![],
            prefixes: BTreeMap::new(),
            compile: BTreeMap::new(),
            roots: roots.to_vec(),
        };
        for root in roots.iter().filter(|r| r.language == Language::Go) {
            let mut parent = root.root.as_path();
            loop {
                let path = parent.join("go.mod").to_string_lossy().into_owned();
                if let Some(bytes) = result.read_input(&path)? {
                    let text =
                        std::str::from_utf8(&bytes).map_err(|_| failure("go.mod is not UTF-8"))?;
                    let modules = text
                        .lines()
                        .filter_map(|line| {
                            let mut words = line.split_whitespace();
                            (words.next() == Some("module"))
                                .then(|| words.next().unwrap_or("").to_owned())
                        })
                        .collect::<Vec<_>>();
                    let [module] = modules.as_slice() else {
                        return Err(failure("go.mod requires one module directive"));
                    };
                    let module = module
                        .split_whitespace()
                        .next()
                        .ok_or_else(|| failure("empty Go module"))?
                        .trim_matches('"');
                    let mut prefix = module.split('/').map(str::to_owned).collect::<Vec<_>>();
                    if prefix
                        .iter()
                        .any(|p| p.is_empty() || matches!(p.as_str(), "." | ".."))
                    {
                        return Err(failure("invalid Go module path"));
                    }
                    prefix.extend(
                        root.root
                            .strip_prefix(parent)
                            .map_err(|_| failure("module root mismatch"))?
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned()),
                    );
                    if root.module_prefix.is_empty() {
                        result.prefixes.insert(root.root.clone(), prefix);
                    }
                    break;
                }
                let Some(next) = parent.parent() else {
                    break;
                };
                parent = next;
            }
        }
        if roots
            .iter()
            .any(|r| matches!(r.language, Language::C | Language::Cpp))
            && let Some(bytes) = result.read_input("compile_commands.json")?
        {
            let commands: Vec<CompileCommand> = serde_json::from_slice(&bytes)
                .map_err(|_| failure("invalid compile_commands.json"))?;
            for command in commands {
                result.add_command(command)?;
            }
        }
        result.inputs.sort_by(|a, b| a.path.cmp(&b.path));
        result.inputs.dedup();
        Ok(result)
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    fn read_input(&mut self, path: &str) -> EngineResult<Option<Vec<u8>>> {
        let value = contents(&self.workspace, path)?;
        self.inputs.push(ConfigurationInput {
            path: path.into(),
            digest: value.as_ref().map(|v| crate::utils::sha256_hex(v)),
        });
        Ok(value)
    }
    #[cfg(any(test, feature = "knowledge-graph-graphdb"))]
    fn add_command(&mut self, command: CompileCommand) -> EngineResult<()> {
        let directory = self
            .workspace
            .join(&command.directory)
            .canonicalize()
            .map_err(|e| EngineError::from_io("resolve compile directory", &e))?;
        if !directory.starts_with(&self.workspace) {
            return Err(failure("compile directory outside workspace"));
        }
        let file = lexical(&directory.join(&command.file));
        let relative = file
            .strip_prefix(&self.workspace)
            .map_err(|_| failure("compile source outside workspace"))?
            .to_string_lossy()
            .replace('\\', "/");
        if !self.roots.iter().any(|r| {
            matches!(r.language, Language::C | Language::Cpp)
                && Path::new(&relative).starts_with(&r.root)
        }) {
            return Ok(());
        }
        let args = match (command.arguments, command.command) {
            (Some(args), _) if !args.is_empty() => args,
            (None, Some(command)) => words(&command)?,
            _ => return Err(failure("compiler arguments missing")),
        };
        let settings = settings(&directory, &args)?;
        match self.compile.get_mut(&relative) {
            Some(previous) if *previous != settings => {
                previous.issue = Some("conflicting_compile_commands".into());
            }
            None => {
                self.compile.insert(relative, settings);
            }
            _ => (),
        }
        Ok(())
    }
    fn for_file(&self, file: &str) -> CompileSettings {
        if let Some(settings) = self.compile.get(file) {
            return settings.clone();
        }
        if !matches!(
            Path::new(file).extension().and_then(|e| e.to_str()),
            Some("h" | "H" | "hh" | "hpp" | "hxx")
        ) {
            return CompileSettings::default();
        }
        let root = self
            .roots
            .iter()
            .filter(|r| Path::new(file).starts_with(&r.root))
            .max_by_key(|r| r.root.components().count());
        let mut candidates = self
            .compile
            .iter()
            .filter(|(path, _)| root.is_some_and(|r| Path::new(path.as_str()).starts_with(&r.root)))
            .map(|(_, s)| s);
        let Some(first) = candidates.next() else {
            return CompileSettings::default();
        };
        if candidates.any(|next| next != first) {
            CompileSettings {
                issue: Some("header_compile_context_ambiguous".into()),
                ..Default::default()
            }
        } else {
            first.clone()
        }
    }
    pub fn apply(&self, facts: &mut FileFacts) -> EngineResult<()> {
        if matches!(facts.language, Language::C | Language::Cpp) {
            let settings = self.for_file(&facts.version.path);
            if settings
                .language
                .is_some_and(|language| language != facts.language)
            {
                facts.complete = false;
                facts
                    .diagnostics
                    .push(super::super::relations::FactDiagnostic {
                        code: "compile_language_conflict".into(),
                        span: None,
                    });
            }
            if let Some(issue) = settings.issue {
                facts.complete = false;
                facts
                    .diagnostics
                    .push(super::super::relations::FactDiagnostic {
                        code: issue,
                        span: None,
                    });
            }
            for name in settings.macros {
                facts.bindings.push(Binding {
                    scope: 0,
                    name,
                    target: BindingTarget::Dynamic,
                    visible_from: 0,
                    visibility: String::new(),
                });
            }
            for reference in &mut facts.references {
                let Lookup::Include {
                    path,
                    system,
                    target,
                } = &mut reference.lookup
                else {
                    continue;
                };
                let mut directories = Vec::new();
                if !*system {
                    directories.push(
                        self.workspace
                            .join(&facts.version.path)
                            .parent()
                            .expect("source parent")
                            .to_path_buf(),
                    );
                    directories.extend(settings.quote.clone());
                }
                directories.extend(settings.include.clone());
                directories.extend(settings.system.clone());
                directories.extend(settings.after.clone());
                for directory in directories {
                    let candidate = directory.join(&*path);
                    match std::fs::metadata(&candidate) {
                        Ok(metadata) if metadata.is_file() => {
                            let canonical = candidate
                                .canonicalize()
                                .map_err(|e| EngineError::from_io("resolve include target", &e))?;
                            *target = canonical
                                .strip_prefix(&self.workspace)
                                .ok()
                                .map(|p| p.to_string_lossy().replace('\\', "/"));
                            break;
                        }
                        Ok(_) => return Err(failure("include target is not a file")),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                        Err(e) => return Err(EngineError::from_io("inspect include target", &e)),
                    }
                }
            }
        }
        Ok(())
    }
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
fn lexical(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => (),
            Component::ParentDir => {
                output.pop();
            }
            part => output.push(part.as_os_str()),
        }
    }
    output
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
fn settings(directory: &Path, args: &[String]) -> EngineResult<CompileSettings> {
    let mut output = CompileSettings::default();
    let mut args = args.iter().skip(1);
    while let Some(argument) = args.next() {
        let argument = argument.as_str();
        if matches!(argument, "-I" | "-isystem" | "-iquote" | "-idirafter") {
            let value = args
                .next()
                .ok_or_else(|| failure("include flag value missing"))?;
            let path = lexical(&directory.join(value));
            match argument {
                "-iquote" => output.quote.push(path),
                "-I" => output.include.push(path),
                "-isystem" => output.system.push(path),
                _ => output.after.push(path),
            }
        } else if let Some(value) = argument.strip_prefix("-I").filter(|v| !v.is_empty()) {
            output.include.push(lexical(&directory.join(value)));
        } else if let Some((flag, value)) = ["-iquote", "-isystem", "-idirafter"]
            .into_iter()
            .find_map(|flag| {
                argument
                    .strip_prefix(flag)
                    .filter(|v| !v.is_empty())
                    .map(|v| (flag, v))
            })
        {
            let path = lexical(&directory.join(value));
            match flag {
                "-iquote" => output.quote.push(path),
                "-isystem" => output.system.push(path),
                _ => output.after.push(path),
            }
        } else if argument == "-x" {
            output.language = Some(match args.next().map(String::as_str) {
                Some("c" | "c-header") => Language::C,
                Some("c++" | "c++-header") => Language::Cpp,
                _ => return Err(failure("unsupported compiler language")),
            });
        } else if argument.starts_with("-D") || argument.starts_with("-U") {
            let value = if argument.len() == 2 {
                args.next()
                    .ok_or_else(|| failure("macro flag value missing"))?
                    .as_str()
            } else {
                &argument[2..]
            };
            let name = value
                .split(['=', '('])
                .next()
                .ok_or_else(|| failure("macro name missing"))?;
            if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err(failure("invalid compiler macro"));
            }
            output
                .macro_values
                .push(format!("{}{}", &argument[..2], value));
            if argument.starts_with("-D") {
                if !output.macros.iter().any(|n| n == name) {
                    output.macros.push(name.into());
                }
            } else {
                output.macros.retain(|n| n != name);
            }
        } else if argument.starts_with('@')
            || ["-include", "-imacros", "-Wp,", "-Xpreprocessor", "-fmodule"]
                .iter()
                .any(|flag| argument.starts_with(flag))
        {
            output.issue = Some("compiler_preprocessing_option_unsupported".into());
        }
    }
    Ok(output)
}
#[cfg(any(test, feature = "knowledge-graph-graphdb"))]
fn words(value: &str) -> EngineResult<Vec<String>> {
    let mut output = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for character in value.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if matches!(character, '$' | '`') && quote != Some('\'') {
            return Err(failure("shell expansion in compiler command"));
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                word.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                started = true;
            }
            '$' | '`' | '|' | '&' | ';' | '<' | '>' => {
                return Err(failure("shell expansion or operators in compiler command"));
            }
            c if c.is_whitespace() => {
                if started {
                    output.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if escaped || quote.is_some() {
        return Err(failure("unterminated compiler command quoting"));
    }
    if started {
        output.push(word);
    }
    if output.is_empty() {
        return Err(failure("empty compiler command"));
    }
    Ok(output)
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;
