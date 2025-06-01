//! The parser for the Koca build format.
//!
//! This doesn't do any static checking outside of ensuring a build file only contains the following in the root level:
//! - Variable assignments
//! - Function declarations
use std::collections::HashMap;

use brush_parser::ast::{
    AssignmentName, AssignmentValue, Command, CommandPrefixOrSuffixItem, CompoundListItem,
    FunctionDefinition, Program, Word,
};

use crate::{KocaError, KocaParserError, KocaResult};

/// The declaration of a variable value.
pub enum DeclValue {
    /// A variable assigned to a string.
    String(Word),
    /// A variable assigned to an array.
    Array(Vec<Word>),
}

impl DeclValue {
    /// If the [`DeclValue`] is a word, return the word. Otherwise, returns [`None`].
    pub fn as_word(&self) -> Option<&Word> {
        match self {
            DeclValue::String(word) => Some(word),
            DeclValue::Array(_) => None,
        }
    }

    /// If the [`DeclValue`] is an array, return the array. Otherwise, returns [`None`].
    pub fn as_array(&self) -> Option<&[Word]> {
        match self {
            DeclValue::String(_) => None,
            DeclValue::Array(array) => Some(array),
        }
    }
}

/// The items that were declared in the currrent [`CompoundListItem`].
///
/// To get this from a [`CompoundListItem`], use [`Decl::try_from`].
enum Decl {
    /// Declared variables, in a tuple of `key` and `value`.
    Vars(HashMap<String, DeclValue>),
    /// A function was declared.
    Func(FunctionDefinition),
}

/// The items found in the list of [`CompoundListItem`]s.
pub struct DeclItems {
    /// The declared variables.
    pub vars: HashMap<String, DeclValue>,
    /// The functions declared.
    pub funcs: Vec<FunctionDefinition>,
}

impl TryFrom<&CompoundListItem> for Decl {
    type Error = KocaError;

    /// Ensure a [`CompoundListItem`] is valid for our use cases, ensuring that it only contains:
    /// - String variable assignments (`var=me`).
    /// - Index array assignments (`var=(1 2 3)`).
    /// - Function definitions.
    /// - Non-duplicate variable assignments.
    ///
    /// Anything outside of the above will trigger a [`KocaError::Parser`] error.
    fn try_from(item: &CompoundListItem) -> KocaResult<Self> {
        let top_level_err = || Err(KocaParserError::TopLevelCommand(item.to_string()).into());

        // Usage of '&&' or '||'.
        if !item.0.additional.is_empty() {
            return top_level_err();
        }

        // Usage of '|'.
        let pipeline = &item.0.first;
        if pipeline.seq.len() > 1 {
            return top_level_err();
        }

        // Usage of 'time' or '!'.
        if pipeline.bang || pipeline.timed.is_some() {
            return top_level_err();
        }

        // Check for any command arguments.
        let cmd = pipeline
            .seq
            .first()
            .expect("pipeline should always contain a command");

        let simple_cmd = match cmd {
            Command::Simple(simple_cmd) => simple_cmd,
            Command::Compound(_, _) => return top_level_err(),
            Command::Function(func) => return Ok(Self::Func(func.to_owned())),
            Command::ExtendedTest(_) => return top_level_err(),
        };

        if simple_cmd.word_or_name.is_some() || simple_cmd.suffix.is_some() {
            return top_level_err();
        }

        let prefix = simple_cmd
            .prefix
            .as_ref()
            .expect("prefix should be present at this point");
        let mut assignments = HashMap::new();

        for prefix_item in &prefix.0 {
            let assignment = match prefix_item {
                CommandPrefixOrSuffixItem::IoRedirect(_) => return top_level_err(),
                CommandPrefixOrSuffixItem::Word(_) => {
                    unreachable!("word should not be present on suffixes")
                }
                CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => assignment.to_owned(),
                CommandPrefixOrSuffixItem::ProcessSubstitution(_, _) => return top_level_err(),
            };
            let invalid_assignment_err =
                || Err(KocaParserError::InvalidAssignment(assignment.clone()).into());

            let name = match &assignment.name {
                AssignmentName::VariableName(name) => name.to_owned(),
                AssignmentName::ArrayElementName(_, _) => return invalid_assignment_err(),
            };

            let value = match &assignment.value {
                AssignmentValue::Scalar(word) => DeclValue::String(word.clone()),
                AssignmentValue::Array(array) => {
                    // Make sure we only allow indexed-arrays.
                    let values: Vec<Word> = array
                        .iter()
                        .filter(|var| var.0.is_none())
                        .map(|var| var.1.clone())
                        .collect();

                    if values.len() != array.len() {
                        return invalid_assignment_err();
                    }

                    DeclValue::Array(values)
                }
            };

            if assignments.insert(name, value).is_some() {
                return Err(KocaParserError::DuplicateAssignment(assignment).into());
            }
        }

        Ok(Decl::Vars(assignments))
    }
}

/// Get all declarations from the [`Program`].
pub fn get_decls(program: &Program) -> KocaResult<DeclItems> {
    let mut items = vec![];

    for line in &program.complete_commands {
        let mut line_items: Vec<&CompoundListItem> = line.0.iter().collect();
        items.append(&mut line_items);
    }

    let mut vars = HashMap::new();
    let mut funcs = vec![];

    for item in items {
        let decl = Decl::try_from(item)?;

        match decl {
            Decl::Vars(decls) => vars.extend(decls),
            Decl::Func(func) => funcs.push(func),
        }
    }

    Ok(DeclItems { vars, funcs })
}
