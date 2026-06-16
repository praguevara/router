#[cfg(test)]
mod introspection_e2e_tests {
    use crate::testkit::{ClientResponseExt, TestRouter};

    #[ntex::test]
    async fn should_work_correctly_for_repeatable_directives() {
        let router = TestRouter::builder()
            .inline_config(&format!(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            ))
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
                query IntrospectionQuery {
                  __schema {
                    directives {
                      name
                      isRepeatable
                    }
                  }
                }"#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");
        let response = resp.json_body_string_pretty().await;

        insta::assert_snapshot!(response, @r###"
        {
          "data": {
            "__schema": {
              "directives": [
                {
                  "name": "test_directive",
                  "isRepeatable": false
                },
                {
                  "name": "test_repeatable_directive",
                  "isRepeatable": true
                },
                {
                  "name": "skip",
                  "isRepeatable": false
                },
                {
                  "name": "include",
                  "isRepeatable": false
                },
                {
                  "name": "deprecated",
                  "isRepeatable": false
                },
                {
                  "name": "specifiedBy",
                  "isRepeatable": false
                },
                {
                  "name": "oneOf",
                  "isRepeatable": false
                }
              ]
            }
          }
        }
        "###);
    }

    #[ntex::test]
    async fn should_have_deprecated_input_values_in_introspection() {
        let router = TestRouter::builder()
            .inline_config(&format!(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            ))
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
            query IncludeDeprecatedInputValues {
                Query: __type(name: "Query") {
                    fields {
                        name
                        args(includeDeprecated: true) {
                            name
                            isDeprecated
                            deprecationReason
                        }
                    }
                }
                TestInput: __type(name: "TestInput") {
                    inputFields(includeDeprecated: true) {
                        name
                        isDeprecated
                        deprecationReason
                    }
                }
                __schema {
                    directives {
                        name
                        args {
                            name
                            isDeprecated
                            deprecationReason
                        }
                    }
                }
            }
        "#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");

        insta::assert_snapshot!(resp.json_body_string_pretty().await, @r###"
        {
          "data": {
            "Query": {
              "fields": [
                {
                  "name": "testField",
                  "args": [
                    {
                      "name": "oldArg",
                      "isDeprecated": true,
                      "deprecationReason": "Use `newArg` instead"
                    },
                    {
                      "name": "newArg",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                }
              ]
            },
            "TestInput": {
              "inputFields": [
                {
                  "name": "oldField",
                  "isDeprecated": true,
                  "deprecationReason": "Use `newField` instead"
                },
                {
                  "name": "newField",
                  "isDeprecated": false,
                  "deprecationReason": null
                }
              ]
            },
            "__schema": {
              "directives": [
                {
                  "name": "test_directive",
                  "args": [
                    {
                      "name": "oldArg",
                      "isDeprecated": true,
                      "deprecationReason": "Use `newArg` instead"
                    },
                    {
                      "name": "newArg",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                },
                {
                  "name": "test_repeatable_directive",
                  "args": [
                    {
                      "name": "oldArg",
                      "isDeprecated": true,
                      "deprecationReason": "Use `newArg` instead"
                    },
                    {
                      "name": "newArg",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                },
                {
                  "name": "skip",
                  "args": [
                    {
                      "name": "if",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                },
                {
                  "name": "include",
                  "args": [
                    {
                      "name": "if",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                },
                {
                  "name": "deprecated",
                  "args": [
                    {
                      "name": "reason",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                },
                {
                  "name": "specifiedBy",
                  "args": [
                    {
                      "name": "url",
                      "isDeprecated": false,
                      "deprecationReason": null
                    }
                  ]
                },
                {
                  "name": "oneOf",
                  "args": []
                }
              ]
            }
          }
        }
        "###);
    }

    #[ntex::test]
    async fn should_have_is_one_of_in_input_values() {
        let router = TestRouter::builder()
            .inline_config(&format!(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            ))
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
            query IncludeOneOfInInputValues {
                TestInput: __type(name: "TestInput") {
                    isOneOf
                }
            }
        "#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");

        insta::assert_snapshot!(resp.json_body_string_pretty().await, @r#"
        {
          "data": {
            "TestInput": {
              "isOneOf": true
            }
          }
        }
        "#);
    }
    #[ntex::test]
    async fn should_have_default_values_in_input_values() {
        let router = TestRouter::builder()
            .inline_config(&format!(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            ))
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
            query IncludeOneOfInInputValues {
                TestInput: __type(name: "TestInput") {
                    inputFields {
                        name
                        defaultValue
                    }
                }
            }
        "#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");

        insta::assert_snapshot!(resp.json_body_string_pretty().await, @r#"
        {
          "data": {
            "TestInput": {
              "inputFields": [
                {
                  "name": "oldField",
                  "defaultValue": null
                },
                {
                  "name": "newField",
                  "defaultValue": "\"newFieldDefaultValue\""
                }
              ]
            }
          }
        }
        "#);
    }
    #[ntex::test]
    async fn should_have_specified_by_url_in_scalar_types() {
        let router = TestRouter::builder()
            .inline_config(&format!(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            ))
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
            query IncludeOneOfInInputValues {
                MyScalar: __type(name: "MyScalar") {
                    specifiedByURL
                }
            }
        "#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");

        insta::assert_snapshot!(resp.json_body_string_pretty().await, @r#"
        {
          "data": {
            "MyScalar": {
              "specifiedByURL": "https://example.com/my-scalar-spec"
            }
          }
        }
        "#);
    }

    #[ntex::test]
    async fn semantic_introspection_definitions_by_coordinate() {
        let router = TestRouter::builder()
            .inline_config(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
semantic_introspection:
                enabled: true
          "#,
            )
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
            {
              __definitions(coordinates: [
                "Query.testField",
                "TestInput",
                "MyScalar",
                "Query.testField.newArg",
                "Does.Not.Exist"
              ]) {
                __typename
                # `name` differs in nullability across members (__Type.name: String
                # vs __Field.name: String!), so alias per member, as a client must.
                ... on __Type { kind typeName: name }
                ... on __Field { fieldName: name fieldType: type { name } args { name } }
                ... on __InputValue { inputName: name inputType: type { name } }
              }
            }
        "#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");
        insta::assert_snapshot!(resp.json_body_string_pretty_stable().await, @r#"
        {
          "data": {
            "__definitions": [
              {
                "__typename": "__Field",
                "args": [
                  {
                    "name": "oldArg"
                  },
                  {
                    "name": "newArg"
                  }
                ],
                "fieldName": "testField",
                "fieldType": {
                  "name": "String"
                }
              },
              {
                "__typename": "__Type",
                "kind": "INPUT_OBJECT",
                "typeName": "TestInput"
              },
              {
                "__typename": "__Type",
                "kind": "SCALAR",
                "typeName": "MyScalar"
              },
              {
                "__typename": "__InputValue",
                "inputName": "newArg",
                "inputType": {
                  "name": "TestInput"
                }
              }
            ]
          }
        }
        "#);
    }

    #[ntex::test]
    async fn semantic_introspection_search_ranks_and_navigates() {
        let router = TestRouter::builder()
            .inline_config(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
semantic_introspection:
                enabled: true
          "#,
            )
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(
                r#"
            {
              __search(query: "test field", first: 3) {
                coordinate
                score
                cursor
                pathsToRoot
                definition {
                  __typename
                  ... on __Field { fieldName: name }
                  ... on __Type { kind typeName: name }
                }
              }
            }
        "#,
                None,
                None,
            )
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");

        // Redact the BM25 score values (the top hit normalizes to 1.0; lower
        // ranks are score-dependent and asserted in the index unit tests). The
        // ranking *order*, coordinates, cursors, paths and union dispatch below
        // are deterministic.
        let body = resp.json_body_string_pretty_stable().await;
        let mut settings = insta::Settings::clone_current();
        settings.add_filter(r#""score": [0-9.]+"#, r#""score": "[score]""#);
        settings.bind(|| {
            insta::assert_snapshot!(body, @r#"
            {
              "data": {
                "__search": [
                  {
                    "coordinate": "TestInput",
                    "cursor": "1",
                    "definition": {
                      "__typename": "__Type",
                      "kind": "INPUT_OBJECT",
                      "typeName": "TestInput"
                    },
                    "pathsToRoot": [],
                    "score": "[score]"
                  },
                  {
                    "coordinate": "Query.testField",
                    "cursor": "2",
                    "definition": {
                      "__typename": "__Field",
                      "fieldName": "testField"
                    },
                    "pathsToRoot": [
                      [
                        "Query.testField"
                      ]
                    ],
                    "score": "[score]"
                  }
                ]
              }
            }
            "#);
        });
    }

    #[ntex::test]
    async fn semantic_introspection_disabled_by_default() {
        // No `semantic_introspection.enabled`, so the feature is off by default.
        let router = TestRouter::builder()
            .inline_config(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            )
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(r#"{ __search(query: "test") { coordinate } }"#, None, None)
            .await;

        assert_eq!(
            resp.status().as_u16(),
            403,
            "semantic introspection must be rejected when disabled"
        );
        insta::assert_snapshot!(resp.json_body_string_pretty_stable().await, @r#"
        {
          "errors": [
            {
              "extensions": {
                "code": "SEMANTIC_INTROSPECTION_DISABLED"
              },
              "message": "Semantic introspection is disabled"
            }
          ]
        }
        "#);
    }

    #[ntex::test]
    async fn regular_introspection_unaffected_when_semantic_disabled() {
        // Plain introspection must still work while semantic introspection is off.
        let router = TestRouter::builder()
            .inline_config(
                r#"supergraph:
                source: file
                path: "./supergraph-introspection-extended.graphql"
          "#,
            )
            .build()
            .start()
            .await;

        let resp = router
            .send_graphql_request(r#"{ __schema { queryType { name } } }"#, None, None)
            .await;

        assert!(resp.status().is_success(), "Expected 200 OK");
        insta::assert_snapshot!(resp.json_body_string_pretty_stable().await, @r#"
        {
          "data": {
            "__schema": {
              "queryType": {
                "name": "Query"
              }
            }
          }
        }
        "#);
    }
}
