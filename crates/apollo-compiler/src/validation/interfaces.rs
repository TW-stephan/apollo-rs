use std::collections::{HashMap, HashSet};

use crate::{
    diagnostics::{
        MissingField, OutputType, RecursiveDefinition, TransitiveImplementedInterfaces,
        UndefinedDefinition, UniqueDefinition, UniqueField,
    },
    hir::{FieldDefinition, InterfaceTypeDefinition},
    validation::ValidationSet,
    ApolloDiagnostic, Document,
};

pub fn check(db: &dyn Document) -> Vec<ApolloDiagnostic> {
    let mut diagnostics = Vec::new();

    // Interface definitions must have unique names.
    //
    // Return a Unique Definition error in case of a duplicate name.
    let mut seen: HashMap<&str, &InterfaceTypeDefinition> = HashMap::new();
    for interface in db.interfaces().iter() {
        let name = interface.name();
        if let Some(prev_def) = seen.get(&name) {
            let prev_offset: usize = prev_def.ast_node(db).text_range().start().into();
            let prev_node_len: usize = prev_def.ast_node(db).text_range().len().into();

            let current_offset: usize = interface.ast_node(db).text_range().start().into();
            let current_node_len: usize = interface.ast_node(db).text_range().len().into();
            diagnostics.push(ApolloDiagnostic::UniqueDefinition(UniqueDefinition {
                ty: "interface".into(),
                name: name.into(),
                src: db.input(),
                original_definition: (prev_offset, prev_node_len).into(),
                redefined_definition: (current_offset, current_node_len).into(),
                help: Some(format!(
                    "`{name}` must only be defined once in this document."
                )),
            }));
        } else {
            seen.insert(name, interface);
        }
    }

    // Interface must not implement itself.
    //
    // Return Recursive Definition error.
    //
    // NOTE(@lrlna): we should also check for more sneaky cyclic references for interfaces like this, for example:
    //
    // interface Node implements Named & Node {
    //   id: ID!
    //   name: String
    // }
    //
    // interface Named implements Node & Named {
    //   id: ID!
    //   name: String
    // }
    for interface_def in db.interfaces().iter() {
        let name = interface_def.name();
        for implements_interface in interface_def.implements_interfaces() {
            if let Some(interface) = implements_interface.interface_definition(db) {
                let i_name = (*interface.name()).to_string();
                if name == i_name {
                    let offset = implements_interface
                        .ast_node(db)
                        .text_range()
                        .start()
                        .into();
                    let len: usize = implements_interface.ast_node(db).text_range().len().into();
                    diagnostics.push(ApolloDiagnostic::RecursiveDefinition(RecursiveDefinition {
                        message: format!("{} interface cannot implement itself", i_name),
                        definition: (offset, len).into(),
                        src: db.input(),
                        definition_label: "recursive implements interfaces".into(),
                    }));
                }
            }
        }
    }

    // Interface Type field validations.
    for interface_def in db.interfaces().iter() {
        let mut seen: HashMap<&str, &FieldDefinition> = HashMap::new();

        let fields = interface_def.fields_definition();

        for field in fields {
            // Fields in an Interface definition must be unique
            //
            // Returns Unique Value error.
            let field_name = field.name();
            let offset: usize = field.ast_node(db).text_range().start().into();
            let len: usize = field.ast_node(db).text_range().len().into();

            if let Some(prev_field) = seen.get(&field_name) {
                let prev_offset: usize = prev_field.ast_node(db).text_range().start().into();
                let prev_node_len: usize = prev_field.ast_node(db).text_range().len().into();

                diagnostics.push(ApolloDiagnostic::UniqueField(UniqueField {
                    field: field_name.into(),
                    src: db.input(),
                    original_field: (prev_offset, prev_node_len).into(),
                    redefined_field: (offset, len).into(),
                    help: Some(format!(
                        "`{field_name}` field must only be defined once in this interface definition."
                    )),
                }));
            } else {
                seen.insert(field_name, field);
            }

            // Field types in interface types must be of output type
            if let Some(field_ty) = field.ty().ty(db) {
                if !field.ty().is_output_type(db) {
                    diagnostics.push(ApolloDiagnostic::OutputType(OutputType {
                        name: field.name().into(),
                        ty: field_ty.ty(),
                        src: db.input(),
                        definition: (offset, len).into(),
                    }))
                }
            } else if let Some(node) = field.ty().ast_node(db) {
                let field_ty_offset: usize = node.text_range().start().into();
                let field_ty_len: usize = node.text_range().len().into();
                diagnostics.push(ApolloDiagnostic::UndefinedDefinition(UndefinedDefinition {
                    ty: field.ty().name(),
                    src: db.input(),
                    definition: (field_ty_offset, field_ty_len).into(),
                }))
            } else {
                diagnostics.push(ApolloDiagnostic::UndefinedDefinition(UndefinedDefinition {
                    ty: field.ty().name(),
                    src: db.input(),
                    definition: (offset, len).into(),
                }))
            }
        }
    }

    let interfaces = db.interfaces();
    let defined_interfaces: HashSet<ValidationSet> = interfaces
        .iter()
        .map(|interface| ValidationSet {
            name: interface.name().to_owned(),
            node: interface.ast_node(db),
        })
        .collect();
    for interface_def in interfaces.iter() {
        // Implements Interfaces must be defined.
        //
        // Returns Undefined Definition error.
        let implements_interfaces: HashSet<ValidationSet> = interface_def
            .implements_interfaces()
            .iter()
            .map(|interface| ValidationSet {
                name: interface.interface().to_owned(),
                node: interface.ast_node(db),
            })
            .collect();
        let diff = implements_interfaces.difference(&defined_interfaces);
        for undefined in diff {
            let offset = undefined.node.text_range().start().into();
            let len: usize = undefined.node.text_range().len().into();
            diagnostics.push(ApolloDiagnostic::UndefinedDefinition(UndefinedDefinition {
                ty: undefined.name.clone(),
                src: db.input(),
                definition: (offset, len).into(),
            }))
        }

        // Transitively implemented interfaces must be defined on an implementing
        // type or interface.
        //
        // Returns Transitive Implemented Interfaces error.
        let transitive_interfaces: HashSet<ValidationSet> = interface_def
            .implements_interfaces()
            .iter()
            .filter_map(|implements_interface| {
                if let Some(interface) = implements_interface.interface_definition(db) {
                    let child_interfaces: HashSet<ValidationSet> = interface
                        .implements_interfaces()
                        .iter()
                        .map(|interface| ValidationSet {
                            name: interface.interface().to_owned(),
                            node: implements_interface.ast_node(db),
                        })
                        .collect();
                    Some(child_interfaces)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        let transitive_diff = transitive_interfaces.difference(&implements_interfaces);
        for undefined in transitive_diff {
            let offset = undefined.node.text_range().start().into();
            let len: usize = undefined.node.text_range().len().into();
            diagnostics.push(ApolloDiagnostic::TransitiveImplementedInterfaces(
                TransitiveImplementedInterfaces {
                    missing_interface: undefined.name.clone(),
                    src: db.input(),
                    definition: (offset, len).into(),
                },
            ))
        }

        // When defining an interface that implements another interface, the
        // implementing interface must define each field that is specified by
        // the implemented interface.
        //
        // Returns a Missing Field error.
        let fields: HashSet<ValidationSet> = interface_def
            .fields_definition()
            .iter()
            .map(|field| ValidationSet {
                name: field.name().into(),
                node: field.ast_node(db),
            })
            .collect();
        for implements_interface in interface_def.implements_interfaces().iter() {
            if let Some(interface) = implements_interface.interface_definition(db) {
                let implements_interface_fields: HashSet<ValidationSet> = interface
                    .fields_definition()
                    .iter()
                    .map(|field| ValidationSet {
                        name: field.name().into(),
                        node: field.ast_node(db),
                    })
                    .collect();

                let field_diff = implements_interface_fields.difference(&fields);

                for missing_field in field_diff {
                    let current_offset: usize =
                        interface_def.ast_node(db).text_range().start().into();
                    let current_len = interface_def.ast_node(db).text_range().len().into();

                    let super_offset = interface.ast_node(db).text_range().start().into();
                    let super_len: usize = interface.ast_node(db).text_range().len().into();

                    diagnostics.push(ApolloDiagnostic::MissingField(MissingField {
                        ty: missing_field.name.clone(),
                        src: db.input(),
                        current_definition: (current_offset, current_len).into(),
                        super_definition: (super_offset, super_len).into(),
                        help: Some(
                            "An interface must be a super-set of all interfaces it implement"
                                .into(),
                        ),
                    }))
                }
            }
        }
    }

    diagnostics
}

#[cfg(test)]
mod test {
    use crate::ApolloCompiler;

    #[test]
    fn it_fails_validation_with_duplicate_operation_fields() {
        let input = r#"
type Query implements NamedEntity {
  imgSize: Int
  name: String
  image: URL
  results: [Int]
}

interface NamedEntity {
  name: String
  image: URL
  results: [Int]
  name: String
}

scalar URL @specifiedBy(url: "https://tools.ietf.org/html/rfc3986")
"#;
        let ctx = ApolloCompiler::new(input);
        let diagnostics = ctx.validate();
        for diagnostic in &diagnostics {
            println!("{}", diagnostic)
        }
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn it_fails_validation_with_duplicate_interface_definitions() {
        let input = r#"
type Query implements NamedEntity {
  imgSize: Int
  name: String
  image: URL
  results: [Int]
}

interface NamedEntity {
  name: String
  image: URL
  results: [Int]
}

interface NamedEntity {
  name: String
  image: URL
  results: [Int]
}

scalar URL @specifiedBy(url: "https://tools.ietf.org/html/rfc3986")
"#;
        let ctx = ApolloCompiler::new(input);
        let diagnostics = ctx.validate();
        for diagnostic in &diagnostics {
            println!("{}", diagnostic)
        }
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn it_fails_validation_with_recursive_interface_definition() {
        let input = r#"
type Query implements NamedEntity {
  imgSize: Int
  name: String
  image: URL
  results: [Int]
}

interface NamedEntity implements NamedEntity {
  name: String
  image: URL
  results: [Int]
}

scalar URL @specifiedBy(url: "https://tools.ietf.org/html/rfc3986")
"#;
        let ctx = ApolloCompiler::new(input);
        let diagnostics = ctx.validate();
        for diagnostic in &diagnostics {
            println!("{}", diagnostic)
        }
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn it_fails_validation_with_undefined_interface_definition() {
        let input = r#"
interface NamedEntity implements NewEntity {
  name: String
  image: URL
  results: [Int]
}

scalar URL @specifiedBy(url: "https://tools.ietf.org/html/rfc3986")
"#;
        let ctx = ApolloCompiler::new(input);
        let diagnostics = ctx.validate();
        for diagnostic in &diagnostics {
            println!("{}", diagnostic)
        }
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn it_fails_validation_with_missing_transitive_interface() {
        let input = r#"
type Query implements Node {
  id: ID!
}

interface Node {
  id: ID!
}

interface Resource implements Node {
  id: ID!
  width: Int
}

interface Image implements Resource & Node {
  id: ID!
  thumbnail: String
}
"#;
        let ctx = ApolloCompiler::new(input);
        let diagnostics = ctx.validate();
        for diagnostic in &diagnostics {
            println!("{}", diagnostic)
        }
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn it_generates_diagnostics_for_non_output_field_types() {
        let input = r#"
query mainPage {
  name
}

type Query {
  name: mainInterface
}

interface mainInterface {
  width: Int
  img: Url
  relationship: Person
  entity: NamedEntity
  depth: Number
  result: SearchResult
  permissions: Auth
  coordinates: Point2D
  main: mainPage
}

type Person {
  name: String
  age: Int
}

type Photo {
  size: Int
  type: String
}

interface NamedEntity {
  name: String
}

enum Number {
  INT
  FLOAT
}

union SearchResult = Photo | Person

directive @Auth(username: String!) repeatable on OBJECT | INTERFACE

input Point2D {
  x: Float
  y: Float
}

scalar Url @specifiedBy(url: "https://tools.ietf.org/html/rfc3986")
"#;
        let ctx = ApolloCompiler::new(input);
        let diagnostics = ctx.validate();
        for diagnostic in &diagnostics {
            println!("{}", diagnostic)
        }
        assert_eq!(diagnostics.len(), 3);
    }
}
