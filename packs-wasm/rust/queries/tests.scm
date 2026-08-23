; Blocks: any module. Modules containing no tests are pruned by the core.
(mod_item
  name: (identifier) @block.name) @block

; Markers: #[test], #[tokio::test], #[test_case(...)], #[rstest] etc.
(attribute_item
  (attribute
    [(identifier) @_attr
     (scoped_identifier) @_attr]
    (#match? @_attr "test"))) @test.marker

; Tests: any function — only counted when preceded by a marker
; (extract.test_requires_marker = true).
(function_item
  name: (identifier) @test.name) @test
