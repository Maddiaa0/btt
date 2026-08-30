; Blocks: describe("...") / suite("..."), including describe.only / .skip,
; plus Playwright-style test.describe("...") / it.describe("...").
; Name captures take the whole string literal; the core decodes it
; (extract.name_syntax = "js-string"), so escaped titles compare by value.
(call_expression
  function: [(identifier) @_block_fn
             (member_expression object: (identifier) @_block_fn)]
  arguments: (arguments . (string) @block.name)
  (#match? @_block_fn "^(describe|suite|{{block_aliases}})$")) @block

(call_expression
  function: (member_expression
    object: (identifier) @_block_obj
    property: (property_identifier) @_block_prop)
  arguments: (arguments . (string) @block.name)
  (#match? @_block_obj "^(it|test)$")
  (#eq? @_block_prop "describe")) @block

; Tests: it("...") / test("...").
(call_expression
  function: (identifier) @_test_fn
  arguments: (arguments . (string) @test.name)
  (#match? @_test_fn "^(it|test|{{test_aliases}})$")) @test

; Test modifiers: it.only / test.skip / it.fails / test.todo / ... — the
; property is constrained so test.describe stays a block, not a test.
(call_expression
  function: (member_expression
    object: (identifier) @_test_fn
    property: (property_identifier) @_test_mod)
  arguments: (arguments . (string) @test.name)
  (#match? @_test_fn "^(it|test|{{test_aliases}})$")
  (#match? @_test_mod "^(only|skip|fails|todo|fixme|slow|concurrent|sequential)$")) @test

; Parameterized tests/blocks are deliberately not named: their generated
; cases cannot be represented as explicit tree leaves. Capturing the `.each`
; member in call position covers both `test.each(table)(...)` and tagged
; tagged-template calls, while restricting the receiver avoids generic
; collection `.each` calls.
(call_expression
  function: (member_expression
    object: (identifier) @_each_obj
    property: (property_identifier) @_each_prop) @unsupported
  (#match? @_each_obj "^(it|test|describe|{{each_aliases}})$")
  (#eq? @_each_prop "each"))

; Alias support is deliberately module-only. This static query collects all
; direct program bindings once; the core resolves chains/cycles in memory and
; compiles at most one alias-expanded query for the file.
(program
  (lexical_declaration
    (variable_declarator
      name: (identifier) @alias.name
      value: (_) @alias.value)))
