// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    anyhow::{format_err, Error},
    fidl_fuchsia_diagnostics::{
        self, ComponentSelector, PropertySelector, Selector, StringSelector, StringSelectorUnknown,
        SubtreeSelector, TreeSelector,
    },
    lazy_static::lazy_static,
    regex::{Regex, RegexSet},
    regex_syntax,
    std::borrow::Borrow,
    std::fs,
    std::io::{BufRead, BufReader},
    std::path::{Path, PathBuf},
};
// Character used to delimit the different sections of an inspect selector,
// the component selector, the tree selector, and the property selector.
pub static SELECTOR_DELIMITER: char = ':';

// Character used to delimit nodes within a component hierarchy path.
static PATH_NODE_DELIMITER: char = '/';

// Character used to escape interperetation of this parser's "special
// characers"; *, /, :, and \.
static ESCAPE_CHARACTER: char = '\\';

// Pattern used to encode wildcard.
pub(crate) static WILDCARD_SYMBOL_STR: &str = "*";
static WILDCARD_SYMBOL_CHAR: char = '*';

static RECURSIVE_WILDCARD_SYMBOL_STR: &str = "**";

// Globs will match everything along a moniker, but won't match empty strings.
static GLOB_REGEX_EQUIVALENT: &str = ".+";

// Wildcards will match anything except for an unescaped slash, since their match
// only extends to a single moniker "node".
//
// It is OK for a wildcard to match nothing when appearing as a pattern match.
// For example, "hello*" matches both "hello world" and "hello".
static WILDCARD_REGEX_EQUIVALENT: &str = r#"(\\/|[^/])*"#;

// Recursive wildcards will match anything, including an unescaped slash.
//
// It is OK for a recursive wildcard to match nothing when appearing in a pattern match.
static RECURSIVE_WILDCARD_REGEX_EQUIVALENT: &str = ".*";

/// Validates a string pattern used in either a PropertySelector or a
/// PathSelectorNode.
/// string patterns:
///    1) Require that the string not be empty.
///    2) Require that the string not contain any
///       glob symbols.
///    3) Require that any escape characters have a matching character they
///       are escaping.
///    4) Require that there are no unescaped selector delimiters, `:`,
///       or unescaped path delimiters, `/`.
fn validate_string_pattern(string_pattern: &str) -> Result<(), Error> {
    lazy_static! {
        static ref STRING_PATTERN_VALIDATOR: RegexSet = RegexSet::new(&[
            // No glob expressions allowed.
            r#"([^\\]\*\*|^\*\*)"#,
            // No unescaped selector delimiters allowed.
            r#"([^\\]:|^:)"#,
            // No unescaped path delimiters allowed.
            r#"([^\\]/|^/)"#,
        ]).unwrap();
    }
    if string_pattern.is_empty() {
        return Err(format_err!("String patterns cannot be empty."));
    }

    let validator_matches = STRING_PATTERN_VALIDATOR.matches(string_pattern);
    if !validator_matches.matched_any() {
        return Ok(());
    } else {
        let mut error_string =
            format!("String pattern {} failed verification: ", string_pattern).to_string();
        if validator_matches.matched(0) {
            error_string.push_str("\n A string pattern cannot contain unescaped glob patterns.");
        }
        if validator_matches.matched(1) {
            error_string
                .push_str("\n A string pattern cannot contain unescaped selector delimiters, `:`.");
        }
        if validator_matches.matched(2) {
            error_string
                .push_str("\n A string pattern cannot contain unescaped path delimiters, `/`.");
        }
        return Err(format_err!("{}", error_string));
    }
}

fn validate_string_selector_allow_recursive_glob(
    string_selector: &StringSelector,
) -> Result<(), Error> {
    match string_selector {
        StringSelector::StringPattern(pattern) if pattern == RECURSIVE_WILDCARD_SYMBOL_STR => {
            Ok(())
        }
        _ => validate_string_selector(string_selector),
    }
}

fn validate_string_selector(string_selector: &StringSelector) -> Result<(), Error> {
    match string_selector {
        StringSelector::StringPattern(pattern) => validate_string_pattern(pattern),
        //TODO(fxbug.dev/4601): What do we need to validate against exact matches?
        StringSelector::ExactMatch(_) => Ok(()),
        _ => Err(format_err!("PathSelectionNodes must be string patterns or pattern matches")),
    }
}

/// Validates all PathSelectorNodes within `path_selection_vector`.
/// PathSelectorNodes:
///     1) Require that all elements of the vector are valid per
///        Selectors::validate_string_pattern specification.
///     2) Require a non-empty vector.
fn validate_tree_path_selection_vector(
    path_selection_vector: &[StringSelector],
) -> Result<(), Error> {
    for path_selection_node in path_selection_vector {
        validate_string_selector(path_selection_node)?;
    }
    Ok(())
}

fn validate_component_path_selection_vector(
    path_selection_vector: &[StringSelector],
) -> Result<(), Error> {
    let last_idx = path_selection_vector.len() - 1;
    for path_selection_node in path_selection_vector[..last_idx].iter() {
        validate_string_selector(path_selection_node)?;
    }
    validate_string_selector_allow_recursive_glob(&path_selection_vector[last_idx])
}

/// Validates a TreeSelector:
/// TreeSelectors:
///    1) Require a present node_path selector field.
///    2) Require that all entries within the node_path are valid per
///       Selectors::validate_node_path specification.
///    3) Require that the target_properties field, if it is present,
///       is valid per Selectors::validate_string_pattern specification.
fn validate_tree_selector(tree_selector: &TreeSelector) -> Result<(), Error> {
    match tree_selector {
        TreeSelector::SubtreeSelector(subtree_selector) => {
            if subtree_selector.node_path.is_empty() {
                return Err(format_err!("Subtree selectors must have non-empty node_path vector."));
            }
            validate_tree_path_selection_vector(&subtree_selector.node_path)?;
        }
        TreeSelector::PropertySelector(property_selector) => {
            if property_selector.node_path.is_empty() {
                return Err(format_err!(
                    "Property selectors must have non-empty node_path vector."
                ));
            }

            validate_tree_path_selection_vector(&property_selector.node_path)?;

            match &property_selector.target_properties {
                StringSelector::StringPattern(pattern) => match validate_string_pattern(pattern) {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(e);
                    }
                },
                StringSelector::ExactMatch(_) => {
                    // TODO(fxbug.dev/4601): What do we need to validate for exact match strings?
                }
                _ => {
                    return Err(format_err!(
                        "target_properties must be either string patterns or exact matches."
                    ))
                }
            }
        }
        _ => return Err(format_err!("TreeSelector only supports property and subtree selection.")),
    }

    Ok(())
}

/// Validates a ComponentSelector:
/// ComponentSelectors:
///    1) Require a present component_moniker field.
///    2) Require that all entries within the component_moniker vector are valid per
///       Selectors::validate_node_path specification.
fn validate_component_selector(component_selector: &ComponentSelector) -> Result<(), Error> {
    match &component_selector.moniker_segments {
        Some(moniker) => {
            if moniker.is_empty() {
                return Err(format_err!(
                    "Component selectors must have non-empty moniker segment vector."
                ));
            }

            validate_component_path_selection_vector(moniker)
        }
        None => Err(format_err!("Component selectors must have a moniker_segment.")),
    }
}

/// Returns true iff a component selector uses the recursive glob.
/// Assumes the selector has already been validated.
pub fn contains_recursive_glob(component_selector: &ComponentSelector) -> bool {
    // Unwrap as a valid selector must contain these fields.
    let last_segment = component_selector.moniker_segments.as_ref().unwrap().last().unwrap();
    match last_segment {
        StringSelector::StringPattern(pattern) if pattern == RECURSIVE_WILDCARD_SYMBOL_STR => true,
        StringSelector::StringPattern(_) => false,
        StringSelector::ExactMatch(_) => false,
        StringSelectorUnknown!() => false,
    }
}

pub fn validate_selector(selector: &Selector) -> Result<(), Error> {
    match (&selector.component_selector, &selector.tree_selector) {
        (Some(component_selector), Some(tree_selector)) => {
            validate_component_selector(component_selector)?;
            validate_tree_selector(tree_selector)?;
            Ok(())
        }
        _ => Err(format_err!("Selectors require a component and tree selector.")),
    }
}

/// Parse a string into a FIDL StringSelector structure.
fn convert_string_to_string_selector(string_to_convert: &str) -> StringSelector {
    // TODO(fxbug.dev/4601): Expose the ability to parse selectors from string into "exact_match" mode.
    StringSelector::StringPattern(string_to_convert.to_string())
}

/// Increments the CharIndices iterator and updates the token builder
/// in order to avoid processing characters being escaped by the selector.
fn handle_escaped_char(
    token_builder: &mut String,
    selection_iter: &mut std::str::CharIndices<'_>,
) -> Result<(), Error> {
    token_builder.push(ESCAPE_CHARACTER);
    let escaped_char_option: Option<(usize, char)> = selection_iter.next();
    match escaped_char_option {
        Some((_, escaped_char)) => token_builder.push(escaped_char),
        None => {
            return Err(format_err!(
                "Selecter fails verification due to unmatched escape character",
            ));
        }
    }
    Ok(())
}

/// Converts a string into a vector of string tokens representing the unparsed
/// string delimited by the provided delimiter, excluded escaped delimiters.
pub fn tokenize_string(untokenized_selector: &str, delimiter: char) -> Result<Vec<String>, Error> {
    let mut token_aggregator = Vec::new();
    let mut curr_token_builder: String = String::new();
    let mut unparsed_selector_iter = untokenized_selector.char_indices();

    while let Some((_, selector_char)) = unparsed_selector_iter.next() {
        match selector_char {
            escape if escape == ESCAPE_CHARACTER => {
                handle_escaped_char(&mut curr_token_builder, &mut unparsed_selector_iter)?;
            }
            selector_delimiter if selector_delimiter == delimiter => {
                if curr_token_builder.is_empty() {
                    return Err(format_err!(
                        "Cannot have empty strings delimited by {}",
                        delimiter
                    ));
                }
                token_aggregator.push(curr_token_builder);
                curr_token_builder = String::new();
            }
            _ => curr_token_builder.push(selector_char),
        }
    }

    // Push the last section of the selector into the aggregator since we don't delimit the
    // end of the selector.
    if curr_token_builder.is_empty() {
        return Err(format_err!(
            "Cannot have empty strings delimited by {}: {}",
            delimiter,
            untokenized_selector
        ));
    }

    token_aggregator.push(curr_token_builder);
    return Ok(token_aggregator);
}

/// Converts an unparsed component selector string into a ComponentSelector.
pub fn parse_component_selector(
    unparsed_component_selector: &str,
) -> Result<ComponentSelector, Error> {
    if unparsed_component_selector.is_empty() {
        return Err(format_err!("ComponentSelector must have atleast one path node.",));
    }

    let tokenized_component_selector =
        tokenize_string(unparsed_component_selector, PATH_NODE_DELIMITER)?;

    let mut component_selector: ComponentSelector = ComponentSelector::EMPTY;

    // Convert every token of the component hierarchy into a PathSelectionNode.
    let path_node_vector = tokenized_component_selector
        .iter()
        .map(|node_string| convert_string_to_string_selector(node_string))
        .collect::<Vec<_>>();

    validate_component_path_selection_vector(&path_node_vector)?;

    component_selector.moniker_segments = Some(path_node_vector);
    return Ok(component_selector);
}

/// Converts an unparsed node path selector and an unparsed property selector into
/// a TreeSelector.
fn parse_tree_selector(
    unparsed_node_path: &str,
    unparsed_property_selector: Option<&str>,
) -> Result<TreeSelector, Error> {
    let node_path_option = if unparsed_node_path.is_empty() {
        None
    } else {
        Some(
            tokenize_string(unparsed_node_path, PATH_NODE_DELIMITER)?
                .iter()
                .map(|node_string| convert_string_to_string_selector(node_string))
                .collect::<Vec<_>>(),
        )
    };

    let property_option = match unparsed_property_selector {
        Some(unparsed_string) => Some(convert_string_to_string_selector(unparsed_string)),
        None => None,
    };

    let tree_selector = match (node_path_option, property_option) {
        (Some(node_path), Some(property)) => TreeSelector::PropertySelector(PropertySelector {
            node_path: node_path,
            target_properties: property,
        }),
        (Some(node_path), None) => {
            TreeSelector::SubtreeSelector(SubtreeSelector { node_path: node_path })
        }
        _ => {
            return Err(format_err!(
                "The provided selector is neither a subtree selector nor a property selector.",
            ))
        }
    };

    validate_tree_selector(&tree_selector)?;
    return Ok(tree_selector);
}

/// Converts an unparsed Inspect selector into a ComponentSelector and TreeSelector.
pub fn parse_selector(unparsed_selector: &str) -> Result<Selector, Error> {
    // Tokenize the selector by `:` char in order to process each subselector separately.
    let selector_sections = tokenize_string(unparsed_selector, SELECTOR_DELIMITER)?;

    match selector_sections.as_slice() {
        [component_selector, inspect_node_selector, property_selector] => Ok(Selector {
            component_selector: Some(parse_component_selector(component_selector)?),
            tree_selector: Some(parse_tree_selector(
                inspect_node_selector,
                Some(property_selector),
            )?),
            ..Selector::EMPTY
        }),
        [component_selector, inspect_node_selector] => Ok(Selector {
            component_selector: Some(parse_component_selector(component_selector)?),
            tree_selector: Some(parse_tree_selector(inspect_node_selector, None)?),
            ..Selector::EMPTY
        }),
        _ => Err(format_err!(
            "Selector format requires at least 2 subselectors delimited by a `:`.",
        )),
    }
}

/// Slice off the end of the input, starting with the first '/' in "//".
fn remove_comments(line: &str) -> Option<&str> {
    const COMMENT: char = '/';

    let mut it = line.chars().enumerate().peekable();
    loop {
        match (it.next(), it.peek()) {
            (Some((start_idx, COMMENT)), Some((_, COMMENT))) => {
                return line.get(0..start_idx);
            }
            (None, _) => return Some(line),
            _ => continue,
        }
    }
}

/// If `line`'s first non-whitespace character is a `"`, then return the substring
/// enclosed by the first " up to the rightmost closing ". If there is no leading ",
/// return `line` unchange. If there is no closing ", return None.
fn remove_quotes(line: &str) -> Option<&str> {
    const QUOTE: char = '"';
    if !line.trim().starts_with(QUOTE) {
        return Some(line);
    }

    line.trim().get(1..line.rfind(QUOTE)?)
}

fn preprocess_line_for_file(line: &str) -> Option<&str> {
    remove_comments(line).map(|line| line.trim()).and_then(remove_quotes)
}

/// Remove any comments process a quoted line.
pub fn parse_selector_file(selector_file: &Path) -> Result<Vec<Selector>, Error> {
    let err = || format_err!("Failed to read line of selector file at configured path.",);
    let selector_file = match fs::File::open(selector_file) {
        Ok(file) => file,
        Err(_) => return Err(format_err!("Failed to open selector file at configured path.")),
    };
    let mut selector_vec = Vec::new();
    let reader = BufReader::new(selector_file);
    for line in reader.lines() {
        match line {
            Ok(line) => {
                let line = preprocess_line_for_file(&line).ok_or(err())?;
                if line.is_empty() {
                    continue;
                }
                selector_vec.push(parse_selector(&line)?);
            }
            Err(_) => return Err(err()),
        }
    }
    Ok(selector_vec)
}

pub fn parse_selectors(selector_path: impl Into<PathBuf>) -> Result<Vec<Selector>, Error> {
    let selector_directory_path: PathBuf = selector_path.into();
    let mut selector_vec: Vec<Selector> = Vec::new();
    for entry in fs::read_dir(selector_directory_path)? {
        let entry = entry?;
        if entry.path().is_dir() {
            return Err(format_err!("Static selector directories are expected to be flat.",));
        } else {
            selector_vec.append(&mut parse_selector_file(&entry.path())?);
        }
    }
    Ok(selector_vec)
}

/// Helper method for converting ExactMatch StringSelectors to regex. We must
/// escape all special characters on the behalf of the selector author when converting
/// exact matches to regex.
fn is_special_character(character: char) -> bool {
    character == ESCAPE_CHARACTER
        || character == PATH_NODE_DELIMITER
        || character == SELECTOR_DELIMITER
        || character == WILDCARD_SYMBOL_CHAR
}

/// Converts a single character from a StringSelector into a format that allows it
/// selected for as a literal character in regular expression. This means that all
/// characters in the selector string, which are also regex meta characters, end up
/// being escaped.
fn convert_single_character_to_regex(token_builder: &mut String, character: char) {
    if regex_syntax::is_meta_character(character) {
        token_builder.push(ESCAPE_CHARACTER);
    }
    token_builder.push(character);
}

/// When the regular expression converter encounters a `\` escape character
/// in the selector string, it needs to express that escape in the regular expression.
/// The regular expression needs to match both the literal backslash and whatever character
/// is being `escaped` in the selector string. So this method converts a selector string
/// like `\:` into `\\:`.
// TODO(fxbug.dev/4601): Should we validate that the only characters being "escaped" in our
//             selector strings are characters that have special syntax in our selector
//             DSL?
fn convert_escaped_char_to_regex(
    token_builder: &mut String,
    selection_iter: &mut std::str::CharIndices<'_>,
) -> Result<(), Error> {
    // We have to push an additional escape for escape characters
    // since the `\` has significance in Regex that we need to escape
    // in order to have literal matching on the backslash.
    token_builder.push(ESCAPE_CHARACTER);
    token_builder.push(ESCAPE_CHARACTER);
    let escaped_char_option: Option<(usize, char)> = selection_iter.next();
    escaped_char_option
        .map(|(_, escaped_char)| convert_single_character_to_regex(token_builder, escaped_char))
        .ok_or(format_err!("Selecter fails verification due to unmatched escape character"))
}

/// Converts a single StringSelector into a regular expression.
///
/// If the StringSelector is a StringPattern, it interperets `\` characters
/// as escape characters that prevent `*` characters from being evaluated as pattern
/// matchers.
///
/// If the StringSelector is an ExactMatch, it will "sanitize" the exact match to
/// align with the format of sanitized text from the system. The resulting regex will
/// be a literal matcher for escape-characters followed by special characters in the
/// selector lanaguage.
fn convert_string_selector_to_regex(
    node: &StringSelector,
    wildcard_symbol_replacement: &str,
    recursive_wildcard_symbol_replacement: Option<&str>,
) -> Result<String, Error> {
    match node {
        StringSelector::StringPattern(string_pattern) => {
            if string_pattern == WILDCARD_SYMBOL_STR {
                Ok(wildcard_symbol_replacement.to_string())
            } else if string_pattern == RECURSIVE_WILDCARD_SYMBOL_STR {
                match recursive_wildcard_symbol_replacement {
                    Some(replacement) => Ok(replacement.to_string()),
                    None => Err(format_err!("Recursive wildcards are not supported")),
                }
            } else {
                let mut node_regex_builder = "(".to_string();
                let mut node_iter = string_pattern.as_str().char_indices();
                while let Some((_, selector_char)) = node_iter.next() {
                    if selector_char == ESCAPE_CHARACTER {
                        convert_escaped_char_to_regex(&mut node_regex_builder, &mut node_iter)?
                    } else if selector_char == WILDCARD_SYMBOL_CHAR {
                        node_regex_builder.push_str(wildcard_symbol_replacement);
                    } else {
                        convert_single_character_to_regex(&mut node_regex_builder, selector_char);
                    }
                }
                node_regex_builder.push_str(")");
                Ok(node_regex_builder)
            }
        }
        StringSelector::ExactMatch(string_pattern) => {
            let mut node_regex_builder = "(".to_string();
            let mut node_iter = string_pattern.as_str().char_indices();
            while let Some((_, selector_char)) = node_iter.next() {
                if is_special_character(selector_char) {
                    // In ExactMatch mode, we assume that the client wants
                    // their series of strings to be a literal match for the
                    // sanitized strings on the system. The sanitized strings
                    // are formed by escaping all special characters, so we do
                    // the same here.
                    node_regex_builder.push(ESCAPE_CHARACTER);
                    node_regex_builder.push(ESCAPE_CHARACTER);
                }
                convert_single_character_to_regex(&mut node_regex_builder, selector_char);
            }
            node_regex_builder.push_str(")");
            Ok(node_regex_builder)
        }
        _ => unreachable!("no expected alternative variants of the path selection node."),
    }
}

/// Converts a vector of StringSelectors into a string capable of constructing a
/// regular expression which matches against strings encoding paths.
///
/// NOTE: The resulting regular expression makes the assumption that all "nodes" in the
/// strings encoding paths that it will match against have been sanitized by the
/// sanitize_string_for_selectors API in this crate.
pub fn convert_path_selector_to_regex(
    selector: &[StringSelector],
    is_subtree_selector: bool,
) -> Result<String, Error> {
    let mut regex_string = "^".to_string();
    for path_selector in selector {
        // Path selectors replace wildcards with a regex that only extends to the next
        // unescaped '/' character, since we want each node to only be applied to one level
        // of the path.
        let node_regex = convert_string_selector_to_regex(
            path_selector,
            WILDCARD_REGEX_EQUIVALENT,
            Some(RECURSIVE_WILDCARD_REGEX_EQUIVALENT),
        )?;
        regex_string.push_str(&node_regex);
        regex_string.push_str("/");
    }

    if is_subtree_selector {
        regex_string.push_str(".*")
    }

    regex_string.push_str("$");

    Ok(regex_string)
}

/// Converts a single StringSelectors into a string capable of constructing a regular
/// expression which matches strings encoding a property name on a node.
///
/// NOTE: The resulting regular expression makes the assumption that the property names
/// that it will match against have been sanitized by the  sanitize_string_for_selectors API in
/// this crate.
pub fn convert_property_selector_to_regex(selector: &StringSelector) -> Result<String, Error> {
    let mut regex_string = "^".to_string();

    // Property selectors replace wildcards with GLOB like behavior since there is no
    // concept of levels/depth to properties.
    let property_regex = convert_string_selector_to_regex(selector, GLOB_REGEX_EQUIVALENT, None)?;
    regex_string.push_str(&property_regex);

    regex_string.push_str("$");

    Ok(regex_string)
}

/// Sanitizes raw strings from the system such that they align with the
/// special-character and escaping semantics of the Selector format.
///
/// Sanitization escapes the known special characters in the selector language.
///
/// NOTE: All strings must be sanitized before being evaluated by
///       selectors in regex form.
pub fn sanitize_string_for_selectors(node: &str) -> String {
    if node.is_empty() {
        return String::new();
    }

    // Preallocate enough space to store the original string.
    let mut sanitized_string = String::with_capacity(node.len());

    node.chars().for_each(|node_char| {
        if is_special_character(node_char) {
            sanitized_string.push(ESCAPE_CHARACTER);
        }
        sanitized_string.push(node_char);
    });

    sanitized_string
}

/// Sanitizes a moniker raw string such that it can be used in a selector.
/// Monikers have a restricted set of characters `a-z`, `0-9`, `_`, `.`, `-`.
/// Each moniker segment is separated by a `\`. Segments for collections also contain `:`.
/// That `:` will be escaped.
pub fn sanitize_moniker_for_selectors(moniker: &str) -> String {
    moniker.replace(":", "\\:")
}

pub fn match_moniker_against_component_selector(
    moniker: &[impl AsRef<str> + std::string::ToString],
    component_selector: &ComponentSelector,
) -> Result<bool, Error> {
    let moniker_selector: &Vec<StringSelector> = match &component_selector.moniker_segments {
        Some(path_vec) => &path_vec,
        None => return Err(format_err!("Component selectors require moniker segments.")),
    };

    let mut sanitized_moniker = moniker
        .iter()
        .map(|s| sanitize_string_for_selectors(s.as_ref()))
        .collect::<Vec<String>>()
        .join("/");

    // We must append a "/" because the regex strings assume that all paths end
    // in a slash.
    sanitized_moniker.push('/');

    let moniker_regex = Regex::new(&convert_path_selector_to_regex(
        moniker_selector,
        /*is_subtree_selector=*/ false,
    )?)?;

    Ok(moniker_regex.is_match(&sanitized_moniker))
}

/// Evaluates a component moniker against a single selector, returning
/// True if the selector matches the component, else false.
///
/// Requires: hierarchy_path is not empty.
///           selectors contains valid Selectors.
pub fn match_component_moniker_against_selector<T>(
    moniker: &[T],
    selector: &Selector,
) -> Result<bool, Error>
where
    T: AsRef<str> + std::string::ToString,
{
    validate_selector(selector)?;

    if moniker.is_empty() {
        return Err(format_err!(
            "Cannot have empty monikers, at least the component name is required."
        ));
    }

    // Unwrap is safe because the validator ensures there is a component selector.
    let component_selector = selector.component_selector.as_ref().unwrap();

    match_moniker_against_component_selector(moniker, component_selector)
}

/// Evaluates a component moniker against a list of selectors, returning
/// all of the selectors which are matches for that moniker.
///
/// Requires: hierarchy_path is not empty.
///           selectors contains valid Selectors.
pub fn match_component_moniker_against_selectors<'a, T>(
    moniker: &[String],
    selectors: &'a [T],
) -> Result<Vec<&'a Selector>, Error>
where
    T: Borrow<Selector>,
{
    if moniker.is_empty() {
        return Err(format_err!(
            "Cannot have empty monikers, at least the component name is required."
        ));
    }

    let selectors = selectors
        .iter()
        .map(|selector| {
            let component_selector = selector.borrow();
            validate_selector(component_selector)?;
            Ok(component_selector)
        })
        .collect::<Result<Vec<&Selector>, Error>>();

    selectors?
        .iter()
        .filter_map(|selector| {
            match_component_moniker_against_selector(moniker, selector)
                .map(|is_match| if is_match { Some(*selector) } else { None })
                .transpose()
        })
        .collect::<Result<Vec<&Selector>, Error>>()
}

/// Evaluates a component moniker against a list of component selectors, returning
/// all of the component selectors which are matches for that moniker.
///
/// Requires: moniker is not empty.
///           component_selectors contains valid ComponentSelectors.
pub fn match_moniker_against_component_selectors<'a, T>(
    moniker: &[String],
    selectors: &'a [T],
) -> Result<Vec<&'a ComponentSelector>, Error>
where
    T: Borrow<ComponentSelector> + 'a,
{
    if moniker.is_empty() {
        return Err(format_err!(
            "Cannot have empty monikers, at least the component name is required."
        ));
    }

    let component_selectors = selectors
        .iter()
        .map(|selector| {
            let component_selector = selector.borrow();
            validate_component_selector(component_selector)?;
            Ok(component_selector)
        })
        .collect::<Result<Vec<&ComponentSelector>, Error>>();

    component_selectors?
        .iter()
        .filter_map(|selector| {
            match_moniker_against_component_selector(moniker, selector)
                .map(|is_match| if is_match { Some(selector.clone()) } else { None })
                .transpose()
        })
        .collect::<Result<Vec<&ComponentSelector>, Error>>()
}

/// Format a |Selector| as a string.
///
/// Returns the formatted |Selector|, or an error if the |Selector| is invalid.
///
/// Note that the output will always include both a component and tree selector. If your input is
/// simply "moniker" you will likely see "moniker:root" as many clients implicitly append "root" if
/// it is not present (e.g. iquery).
pub fn selector_to_string(selector: Selector) -> Result<String, Error> {
    validate_selector(&selector)?;

    let component_selector =
        selector.component_selector.ok_or_else(|| format_err!("component selector missing"))?;
    let (node_path, maybe_property_selector) = match selector
        .tree_selector
        .ok_or_else(|| format_err!("tree selector missing"))?
    {
        TreeSelector::SubtreeSelector(SubtreeSelector { node_path, .. }) => (node_path, None),
        TreeSelector::PropertySelector(PropertySelector {
            node_path, target_properties, ..
        }) => (node_path, Some(target_properties)),
        _ => return Err(format_err!("unknown tree selector type")),
    };

    let mut segments = vec![];

    let escape_special_chars = |val: &str| {
        let mut ret = String::with_capacity(val.len());
        for c in val.chars() {
            if is_special_character(c) {
                ret.push('\\');
            }
            ret.push(c);
        }
        ret
    };

    let process_string_selector_vector = |v: Vec<StringSelector>| -> Result<String, Error> {
        Ok(v.into_iter()
            .map(|segment| match segment {
                StringSelector::StringPattern(s) => Ok(s),
                StringSelector::ExactMatch(s) => Ok(escape_special_chars(&s)),
                _ => {
                    return Err(format_err!("Unknown string selector type"));
                }
            })
            .collect::<Result<Vec<_>, Error>>()?
            .join("/"))
    };

    // Create the component moniker
    segments.push(process_string_selector_vector(
        component_selector
            .moniker_segments
            .ok_or_else(|| format_err!("component selector missing moniker"))?,
    )?);

    // Create the node selector
    segments.push(process_string_selector_vector(node_path)?);

    if let Some(property_selector) = maybe_property_selector {
        segments.push(process_string_selector_vector(vec![property_selector])?);
    }

    Ok(segments.join(":"))
}

/// Matches a string against a single StringSelector.
/// This will only match against a single "level" and does not support recursive globbing.
pub fn match_selector_against_single_node(
    node: &impl AsRef<str>,
    selector: &StringSelector,
) -> Result<bool, Error> {
    let regex = Regex::new(&format!(
        "^{}$",
        convert_string_selector_to_regex(selector, WILDCARD_REGEX_EQUIVALENT, None)?
    ))?;

    Ok(regex.is_match(&sanitize_string_for_selectors(node.as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::prelude::*;
    use tempfile::TempDir;

    // TODO(fxbug.dev/55118): REMOVE. When updating this test, please make sure the one of the same
    // name in parser.rs is updated.
    #[test]
    fn canonical_component_selector_test() {
        let test_vector = vec![
            (
                "a/b/c",
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("b".to_string()),
                StringSelector::StringPattern("c".to_string()),
            ),
            (
                "a/*/c",
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("*".to_string()),
                StringSelector::StringPattern("c".to_string()),
            ),
            (
                "a/b*/c",
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("b*".to_string()),
                StringSelector::StringPattern("c".to_string()),
            ),
            (
                r#"a/b\*/c"#,
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern(r#"b\*"#.to_string()),
                StringSelector::StringPattern("c".to_string()),
            ),
            (
                r#"a/\*/c"#,
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern(r#"\*"#.to_string()),
                StringSelector::StringPattern("c".to_string()),
            ),
            (
                "a/b/**",
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("b".to_string()),
                StringSelector::StringPattern("**".to_string()),
            ),
        ];

        for (test_string, first_path_node, second_path_node, target_component) in test_vector {
            let component_selector = parse_component_selector(&test_string).unwrap();

            match component_selector.moniker_segments.as_ref().unwrap().as_slice() {
                [first, second, third] => {
                    assert_eq!(*first, first_path_node);
                    assert_eq!(*second, second_path_node);
                    assert_eq!(*third, target_component);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn try_remove_comments() {
        let test_cases = vec![
            (Some(r"a:\/\/b:\/c"), r"a:\/\/b:\/c"),
            (Some(r"a/b/c"), r"a/b/c"),
            (Some(""), "// a comment"),
            (Some("a:b:c "), "a:b:c // inline comment"),
            (Some("   "), "   // leading whitespace is not trimmed"),
            (Some("\t\t "), "\t\t // including tabs"),
        ];

        for (case, (expected, actual_input)) in test_cases.into_iter().enumerate() {
            assert_eq!(
                expected,
                remove_comments(actual_input),
                "test case number: {}, raw data: <{}>",
                case,
                actual_input
            );
        }
    }

    #[test]
    fn try_remove_quotes() {
        let test_cases = vec![
            (Some("a:b:c"), r#""a:b:c""#),
            (None, r#""a:b:c"#),
            (Some(r#"a:b":c"#), r#"a:b":c"#),
            (Some("a:b:c  "), r#""a:b:c  ""#),
            (Some(r#"a:"b":"c "#), r##""a:"b":"c ""##),
            (Some("a:b:c"), "a:b:c"),
            (Some("  a:b:c  "), "  a:b:c  "),
            (Some("a: b:c "), "a: b:c "),
        ];

        for (case, (expected, actual_input)) in test_cases.into_iter().enumerate() {
            assert_eq!(
                expected,
                remove_quotes(actual_input),
                "test case number: {}, raw data: <{}>",
                case,
                actual_input
            );
        }
    }

    #[test]
    fn try_preprocess_file_lines() {
        let test_cases = vec![
            (Some("a/b/c"), "a/b/c"),
            (Some("a/b/c"), "a/b/c // a comment"),
            (Some("  hello:world/a/b  "), r#""  hello:world/a/b  ""#),
            (Some(r#"a:/b"c"#), r#"a:/b"c"#),
            (Some(r#"a:b"c:d"#), r#""a:b"c:d"  // a comment"#),
            (Some(""), "//"),
            (None, r#"""#),
        ];

        for (case, (expected, actual_input)) in test_cases.into_iter().enumerate() {
            assert_eq!(
                expected,
                preprocess_line_for_file(actual_input),
                "test case number: {}, raw data: <{}>",
                case,
                actual_input
            );
        }
    }

    // TODO(fxbug.dev/55118): REMOVE. When updating this test, please make sure the one of the same
    // name in parser.rs is updated.
    #[test]
    fn missing_path_component_selector_test() {
        let component_selector_string = "c";
        let component_selector =
            parse_component_selector(&component_selector_string.to_string()).unwrap();
        let mut path_vec = component_selector.moniker_segments.unwrap();
        assert_eq!(path_vec.pop(), Some(StringSelector::StringPattern("c".to_string())));

        assert!(path_vec.is_empty());
    }

    // TODO(fxbug.dev/55118): REMOVE. When updating this test, please make sure the one of the same
    // name in parser.rs is updated.
    #[test]
    fn path_components_have_spaces_as_names_selector_test() {
        let component_selector_string = " ";
        let component_selector =
            parse_component_selector(&component_selector_string.to_string()).unwrap();
        let mut path_vec = component_selector.moniker_segments.unwrap();
        assert_eq!(path_vec.pop(), Some(StringSelector::StringPattern(" ".to_string())));

        assert!(path_vec.is_empty());
    }

    // TODO(fxbug.dev/55118): REMOVE. When updating this test, please make sure the one of the same
    // name in parser.rs is updated.
    #[test]
    fn errorful_component_selector_test() {
        let test_vector: Vec<String> = vec![
            "".to_string(),
            "a\\".to_string(),
            r#"a/b***/c"#.to_string(),
            r#"a/***/c"#.to_string(),
            r#"a/**/c"#.to_string(),
            // supported? r#"a/*/b/**"#.to_string(),
        ];
        for test_string in test_vector {
            let component_selector_result = parse_component_selector(&test_string);
            assert!(component_selector_result.is_err());
        }
    }

    // TODO(fxbug.dev/55118): REMOVE. When updating this test, please make sure the one of the same
    // name in parser.rs is updated.
    #[test]
    fn canonical_tree_selector_test() {
        let test_vector = vec![
            (
                "a/b",
                Some("c"),
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("b".to_string()),
                Some(StringSelector::StringPattern("c".to_string())),
            ),
            (
                "a/*",
                Some("c"),
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("*".to_string()),
                Some(StringSelector::StringPattern("c".to_string())),
            ),
            (
                "a/b",
                Some("*"),
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("b".to_string()),
                Some(StringSelector::StringPattern("*".to_string())),
            ),
            (
                "a/b",
                None,
                StringSelector::StringPattern("a".to_string()),
                StringSelector::StringPattern("b".to_string()),
                None,
            ),
        ];

        for (
            test_node_path,
            test_target_property,
            first_path_node,
            second_path_node,
            parsed_property,
        ) in test_vector
        {
            let tree_selector = parse_tree_selector(test_node_path, test_target_property).unwrap();
            match tree_selector {
                TreeSelector::SubtreeSelector(tree_selector) => {
                    match tree_selector.node_path.as_slice() {
                        [first, second] => {
                            assert_eq!(*first, first_path_node);
                            assert_eq!(*second, second_path_node);
                        }
                        _ => unreachable!(),
                    }
                }
                TreeSelector::PropertySelector(tree_selector) => {
                    assert_eq!(tree_selector.target_properties, parsed_property.unwrap());
                    match tree_selector.node_path.as_slice() {
                        [first, second] => {
                            assert_eq!(*first, first_path_node);
                            assert_eq!(*second, second_path_node);
                        }
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    // TODO(fxbug.dev/55118): REMOVE. When updating this test, please make sure the one of the same
    // name in parser.rs is updated.
    #[test]
    fn errorful_tree_selector_test() {
        let test_vector = vec![
            // Not allowed due to empty property selector.
            ("a/b", Some("")),
            // Not allowed due to glob property selector.
            ("a/b", Some("**")),
            // Not allowed due to escape-char without a thing to escape.
            (r#"a/b\"#, Some("c")),
            // String literals can't have globs.
            (r#"a/b**"#, Some("c")),
            // Property selector string literals cant have globs.
            (r#"a/b"#, Some("c**")),
            ("a/b", Some("**")),
            // Node path cant have globs.
            ("a/**", Some("c")),
            ("", Some("c")),
        ];
        for (test_nodepath, test_target_property) in test_vector {
            let tree_selector_result = parse_tree_selector(test_nodepath, test_target_property);
            assert!(tree_selector_result.is_err());
        }
    }

    #[test]
    fn successful_selector_parsing() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        File::create(tempdir.path().join("a.txt"))
            .expect("create file")
            .write_all(
                b"a:b:c

",
            )
            .expect("writing test file");
        File::create(tempdir.path().join("b.txt"))
            .expect("create file")
            .write_all(b"a*/b:c/d/*:*")
            .expect("writing test file");

        File::create(tempdir.path().join("c.txt"))
            .expect("create file")
            .write_all(
                b"// this is a comment
a:b:c
",
            )
            .expect("writing test file");

        assert!(parse_selectors(tempdir.path()).is_ok());
    }

    #[test]
    fn unsuccessful_selector_parsing_bad_selector() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        File::create(tempdir.path().join("a.txt"))
            .expect("create file")
            .write_all(b"a:b:c")
            .expect("writing test file");
        File::create(tempdir.path().join("b.txt"))
            .expect("create file")
            .write_all(b"**:**:**")
            .expect("writing test file");

        assert!(parse_selectors(tempdir.path()).is_err());
    }

    #[test]
    fn unsuccessful_selector_parsing_nonflat_dir() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        File::create(tempdir.path().join("a.txt"))
            .expect("create file")
            .write_all(b"a:b:c")
            .expect("writing test file");
        File::create(tempdir.path().join("b.txt"))
            .expect("create file")
            .write_all(b"**:**:**")
            .expect("writing test file");

        std::fs::create_dir_all(tempdir.path().join("nested")).expect("make nested");
        File::create(tempdir.path().join("nested/c.txt"))
            .expect("create file")
            .write_all(b"**:**:**")
            .expect("writing test file");
        assert!(parse_selectors(tempdir.path()).is_err());
    }

    #[test]
    fn canonical_path_regex_transpilation_test() {
        // Note: We provide the full selector syntax but this test is only transpiling
        // the node-path of the selector, and validating against that.
        let test_cases = vec![
            (r#"echo.cmx:a/*/c:*"#, vec!["a", "b", "c"]),
            (r#"echo.cmx:a/*/*/*/*/c:*"#, vec!["a", "b", "g", "e", "d", "c"]),
            (r#"echo.cmx:*/*/*/*/*/*/*:*"#, vec!["a", "b", "/c", "d", "e*", "f"]),
            (r#"echo.cmx:a/*/*/d/*/*:*"#, vec!["a", "b", "/c", "d", "e*", "f"]),
            (r#"echo.cmx:a/*/\/c/d/e\*/*:*"#, vec!["a", "b", "/c", "d", "e*", "f"]),
            (r#"echo.cmx:a/b*/c:*"#, vec!["a", "bob", "c"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b", "c"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b", "c", "/"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b", "c", "d"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b", "c", "d", "e"]),
        ];
        for (selector, string_to_match) in test_cases {
            let mut sanitized_node_path = string_to_match
                .iter()
                .map(|s| sanitize_string_for_selectors(s))
                .collect::<Vec<String>>()
                .join("/");
            // We must append a "/" because the absolute monikers end in slash and
            // hierarchy node paths don't, but we want to reuse the regex logic.
            sanitized_node_path.push('/');

            let parsed_selector = parse_selector(selector).unwrap();
            let tree_selector = parsed_selector.tree_selector.unwrap();
            match tree_selector {
                TreeSelector::SubtreeSelector(tree_selector) => {
                    let node_path = tree_selector.node_path;
                    let selector_regex =
                        Regex::new(&convert_path_selector_to_regex(&node_path, true).unwrap())
                            .unwrap();
                    assert!(selector_regex.is_match(&sanitized_node_path));
                }
                TreeSelector::PropertySelector(tree_selector) => {
                    let node_path = tree_selector.node_path;
                    let selector_regex =
                        Regex::new(&convert_path_selector_to_regex(&node_path, false).unwrap())
                            .unwrap();
                    assert!(selector_regex.is_match(&sanitized_node_path));
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn failing_path_regex_transpilation_test() {
        // Note: We provide the full selector syntax but this test is only transpiling
        // the node-path of the tree selector, and valdating against that.
        let test_cases = vec![
            // Failing because it's missing a required "d" directory node in the string.
            (r#"echo.cmx:a/*/d/*/f:*"#, vec!["a", "b", "c", "e", "f"]),
            // Failing because the match string doesnt end at the c node.
            (r#"echo.cmx:a/*/*/*/*/*/c:*"#, vec!["a", "b", "g", "e", "d", "f"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b", "card"]),
            (r#"echo.cmx:a/b/c"#, vec!["a", "b", "c/"]),
        ];
        for (selector, string_to_match) in test_cases {
            let mut sanitized_node_path = string_to_match
                .iter()
                .map(|s| sanitize_string_for_selectors(s))
                .collect::<Vec<String>>()
                .join("/");
            // We must append a "/" because the absolute monikers end in slash and
            // hierarchy node paths don't, but we want to reuse the regex logic.
            sanitized_node_path.push('/');

            let parsed_selector = parse_selector(selector).unwrap();
            let tree_selector = parsed_selector.tree_selector.unwrap();
            match tree_selector {
                TreeSelector::SubtreeSelector(tree_selector) => {
                    let node_path = tree_selector.node_path;
                    let selector_regex =
                        Regex::new(&convert_path_selector_to_regex(&node_path, true).unwrap())
                            .unwrap();
                    assert!(!selector_regex.is_match(&sanitized_node_path));
                }
                TreeSelector::PropertySelector(tree_selector) => {
                    let node_path = tree_selector.node_path;
                    let selector_regex =
                        Regex::new(&convert_path_selector_to_regex(&node_path, false).unwrap())
                            .unwrap();
                    assert!(!selector_regex.is_match(&sanitized_node_path));
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn canonical_property_regex_transpilation_test() {
        // Note: We provide the full selector syntax but this test is only transpiling
        // the property of the selector, and validating against that.
        let test_cases = vec![
            (r#"echo.cmx:a:*"#, r#"a"#),
            (r#"echo.cmx:a:bob"#, r#"bob"#),
            (r#"echo.cmx:a:b*"#, r#"bob"#),
            (r#"echo.cmx:a:\*"#, r#"*"#),
        ];
        for (selector, string_to_match) in test_cases {
            let parsed_selector = parse_selector(selector).unwrap();
            let tree_selector = parsed_selector.tree_selector.unwrap();
            match tree_selector {
                TreeSelector::SubtreeSelector(_) => {
                    unreachable!("Subtree selectors don't test property selection.")
                }
                TreeSelector::PropertySelector(tree_selector) => {
                    let property_selector = tree_selector.target_properties;
                    let selector_regex = Regex::new(
                        &convert_property_selector_to_regex(&property_selector).unwrap(),
                    )
                    .unwrap();
                    assert!(
                        selector_regex.is_match(&sanitize_string_for_selectors(string_to_match))
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn failing_property_regex_transpilation_test() {
        // Note: We provide the full selector syntax but this test is only transpiling
        // the node-path of the tree selector, and valdating against that.
        let test_cases = vec![
            (r#"echo.cmx:a:c"#, r#"d"#),
            (r#"echo.cmx:a:bob"#, r#"thebob"#),
            (r#"echo.cmx:a:c"#, r#"cdog"#),
        ];
        for (selector, string_to_match) in test_cases {
            let parsed_selector = parse_selector(selector).unwrap();
            let tree_selector = parsed_selector.tree_selector.unwrap();
            match tree_selector {
                TreeSelector::SubtreeSelector(_) => {
                    unreachable!("Subtree selectors don't test property selection.")
                }
                TreeSelector::PropertySelector(tree_selector) => {
                    let target_properties = tree_selector.target_properties;
                    let selector_regex = Regex::new(
                        &convert_property_selector_to_regex(&target_properties).unwrap(),
                    )
                    .unwrap();
                    assert!(
                        !selector_regex.is_match(&sanitize_string_for_selectors(string_to_match))
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    lazy_static! {
        static ref SHARED_PASSING_TEST_CASES: Vec<(Vec<&'static str>, &'static str)> = {
            vec![
                (vec![r#"abc"#, r#"def"#, r#"g"#], r#"bob"#),
                (vec![r#"\**"#], r#"\**"#),
                (vec![r#"\/"#], r#"\/"#),
                (vec![r#"\:"#], r#"\:"#),
                (vec![r#"asda\\\:"#], r#"a"#),
                (vec![r#"asda*"#], r#"a"#),
            ]
        };
        static ref SHARED_FAILING_TEST_CASES: Vec<(Vec<&'static str>, &'static str)> = {
            vec![
                // Slashes aren't allowed in path nodes.
                (vec![r#"/"#], r#"a"#),
                // Colons aren't allowed in path nodes.
                (vec![r#":"#], r#"a"#),
                // Checking that path nodes ending with offlimits
                // chars are still identified.
                (vec![r#"asdasd:"#], r#"a"#),
                (vec![r#"a**"#], r#"a"#),
                // Checking that path nodes starting with offlimits
                // chars are still identified.
                (vec![r#":asdasd"#], r#"a"#),
                (vec![r#"**a"#], r#"a"#),
                // Neither moniker segments nor node paths
                // are allowed to be empty.
                (vec![], r#"bob"#),
            ]
        };
    }

    #[test]
    fn tree_selector_validator_test() {
        let unique_failing_test_cases = vec![
            // All failing validators due to property selectors are
            // unique since the component validator doesnt look at them.
            (vec![r#"a"#], r#"**"#),
            (vec![r#"a"#], r#"/"#),
        ];

        fn create_tree_selector(node_path: &Vec<&str>, property: &str) -> TreeSelector {
            let node_path = node_path
                .iter()
                .map(|path_node_str| StringSelector::StringPattern(path_node_str.to_string()))
                .collect::<Vec<StringSelector>>();
            let target_properties = StringSelector::StringPattern(property.to_string());
            TreeSelector::PropertySelector(PropertySelector { node_path, target_properties })
        }

        for (node_path, property) in SHARED_PASSING_TEST_CASES.iter() {
            let tree_selector = create_tree_selector(node_path, property);
            assert!(validate_tree_selector(&tree_selector).is_ok());
        }

        for (node_path, property) in SHARED_FAILING_TEST_CASES.iter() {
            let tree_selector = create_tree_selector(node_path, property);
            assert!(
                validate_tree_selector(&tree_selector).is_err(),
                "Failed to validate tree selector: {:?}",
                tree_selector
            );
        }

        for (node_path, property) in unique_failing_test_cases.iter() {
            let tree_selector = create_tree_selector(node_path, property);
            assert!(
                validate_tree_selector(&tree_selector).is_err(),
                "Failed to validate tree selector: {:?}",
                tree_selector
            );
        }
    }

    #[test]
    fn component_selector_validator_test() {
        fn create_component_selector(component_moniker: &Vec<&str>) -> ComponentSelector {
            let mut component_selector = ComponentSelector::EMPTY;
            component_selector.moniker_segments = Some(
                component_moniker
                    .into_iter()
                    .map(|path_node_str| StringSelector::StringPattern(path_node_str.to_string()))
                    .collect::<Vec<StringSelector>>(),
            );
            component_selector
        }

        for (component_moniker, _) in SHARED_PASSING_TEST_CASES.iter() {
            let component_selector = create_component_selector(component_moniker);

            assert!(validate_component_selector(&component_selector).is_ok());
        }

        for (component_moniker, _) in SHARED_FAILING_TEST_CASES.iter() {
            let component_selector = create_component_selector(component_moniker);

            assert!(
                validate_component_selector(&component_selector).is_err(),
                "Failed to validate component selector: {:?}",
                component_selector
            );
        }
    }

    #[test]
    fn component_selector_match_test() {
        // Note: We provide the full selector syntax but this test is only validating it
        // against the provided moniker
        let passing_test_cases = vec![
            (r#"echo.cmx:*:*"#, vec!["echo.cmx"]),
            (r#"*/echo.cmx:*:*"#, vec!["abc", "echo.cmx"]),
            (r#"ab*/echo.cmx:*:*"#, vec!["abc", "echo.cmx"]),
            (r#"ab*/echo.cmx:*:*"#, vec!["abcde", "echo.cmx"]),
            (r#"*/ab*/echo.cmx:*:*"#, vec!["123", "abcde", "echo.cmx"]),
            (r#"a\/\*/echo.cmx:*:*"#, vec!["a/*", "echo.cmx"]),
            (r#"echo.cmx*:*:*"#, vec!["echo.cmx"]),
            (r#"a/echo*.cmx:*:*"#, vec!["a", "echo1.cmx"]),
            (r#"a/echo*.cmx:*:*"#, vec!["a", "echo.cmx"]),
            (r#"ab*/echo.cmx:*:*"#, vec!["ab", "echo.cmx"]),
            (r#"a/**:*:*"#, vec!["a", "echo.cmx"]),
            (r#"a/**:*:*"#, vec!["a", "b", "echo.cmx"]),
        ];

        for (selector, moniker) in passing_test_cases {
            let parsed_selector = parse_selector(selector).unwrap();
            assert!(
                match_component_moniker_against_selector(&moniker, &parsed_selector).unwrap(),
                "Selector {:?} failed to match {:?}",
                selector,
                moniker
            );
        }

        // Note: We provide the full selector syntax but this test is only validating it
        // against the provided moniker
        let failing_test_cases = vec![
            (r#"*:*:*"#, vec!["a", "echo.cmx"]),
            (r#"*/echo.cmx:*:*"#, vec!["123", "abc", "echo.cmx"]),
            (r#"a/**:*:*"#, vec!["b", "echo.cmx"]),
            (r#"e/**:*:*"#, vec!["echo.cmx"]),
        ];

        for (selector, moniker) in failing_test_cases {
            let parsed_selector = parse_selector(selector).unwrap();
            assert!(
                !match_component_moniker_against_selector(&moniker, &parsed_selector).unwrap(),
                "Selector {:?} matched {:?}, but was expected to fail",
                selector,
                moniker
            );
        }
    }

    #[test]
    fn multiple_component_selectors_match_test() {
        let selectors = vec![r#"*/echo.cmx"#, r#"ab*/echo.cmx"#, r#"abc/m*"#];
        let moniker = vec!["abc".to_string(), "echo.cmx".to_string()];

        let component_selectors = selectors
            .into_iter()
            .map(|selector| parse_component_selector(&selector.to_string()).unwrap())
            .collect::<Vec<_>>();

        let match_res =
            match_moniker_against_component_selectors(moniker.as_slice(), &component_selectors[..]);
        assert!(match_res.is_ok());
        assert_eq!(match_res.unwrap().len(), 2);
    }

    #[test]
    fn selector_to_string_test() {
        // Check that parsing and formatting these selectors results in output identical to the
        // original selector.
        let cases = vec![
            r#"moniker:root"#,
            r#"my/component:root"#,
            r#"my/component:root:a"#,
            r#"a/b/c\*ff:root:a"#,
            r#"a/child*:root:a"#,
            r#"a/child:root/a/b/c"#,
            r#"a/child:root/a/b/c:d"#,
            r#"a/child:root/a/b/c*/d"#,
            r#"a/child:root/a/b/c\*/d"#,
            r#"a/child:root/a/b/c:d*"#,
            r#"a/child:root/a/b/c:*d*"#,
            r#"a/child:root/a/b/c:\*d*"#,
            r#"a/child:root/a/b/c:\*d\:\*\\"#,
        ];

        for input in cases {
            let selector = parse_selector(input)
                .unwrap_or_else(|e| panic!("Failed to parse '{}': {}", input, e));
            let output = selector_to_string(selector).unwrap_or_else(|e| {
                panic!("Failed to format parsed selector for '{}': {}", input, e)
            });
            assert_eq!(output, input);
        }
    }

    #[test]
    fn exact_match_selector_to_string() {
        let selector = Selector {
            component_selector: Some(ComponentSelector {
                moniker_segments: Some(vec![StringSelector::ExactMatch("a*:".to_string())]),
                ..ComponentSelector::EMPTY
            }),
            tree_selector: Some(TreeSelector::SubtreeSelector(SubtreeSelector {
                node_path: vec![StringSelector::ExactMatch("a*:".to_string())],
            })),
            ..Selector::EMPTY
        };

        // Check we generate the expected string with escaping.
        let selector_string = selector_to_string(selector).unwrap();
        assert_eq!(r#"a\*\::a\*\:"#, selector_string);

        // Parse the resultant selector, and check that it matches a moniker it is supposed to.
        let parsed = parse_selector(&selector_string).unwrap();
        assert!(match_moniker_against_component_selector(
            &["a*:"],
            parsed.component_selector.as_ref().unwrap()
        )
        .unwrap());
    }

    #[test]
    fn sanitize_moniker_for_selectors_result_is_usable() {
        let selector =
            parse_selector(&format!("{}:root", sanitize_moniker_for_selectors("foo/coll:bar/baz")))
                .unwrap();
        let component_selector = selector.component_selector.as_ref().unwrap();
        let moniker = vec!["foo".to_string(), "coll:bar".to_string(), "baz".to_string()];
        assert!(match_moniker_against_component_selector(&moniker, &component_selector).unwrap());
    }
}
