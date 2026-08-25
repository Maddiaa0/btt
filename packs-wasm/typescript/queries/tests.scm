; Blocks: describe("...") / suite("..."), including describe.only / .skip,
; plus Playwright-style test.describe("...") / it.describe("...").
; Name captures take the whole string literal; the core decodes it
; (extract.name_syntax = "js-string"), so escaped titles compare by value.
;
; `{{block_aliases}}` / `{{test_aliases}}` are placeholders the core fills
; with the names the @block.alias / @test.alias patterns below discover
; (a match-nothing class while there are none), re-running the query until
; the sets stop growing — so `const describeDb = flag ? describe :
; describe.skip` opens blocks exactly like describe, aliases of aliases
; included.
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

; Alias declarations: `const describeDb = describe`, `= describe.skip`,
; `= flag ? describe : describe.skip` (both arms block-ish), and the same
; shapes for it/test. The recognized forms are mirrored by the lexical
; twin's [lexical.*] alias patterns, which approximate them line-locally —
; keep changes to these shapes in lock-step. Import aliases and
; multi-declarator statements are out of scope.
(variable_declarator
  name: (identifier) @block.alias
  value: [(identifier) (member_expression)] @_balias
  (#match? @_balias "^(describe|suite|{{block_aliases}})(\\s*\\.\\s*\\w+)?$|^(it|test)\\s*\\.\\s*describe$"))

(variable_declarator
  name: (identifier) @block.alias
  value: (ternary_expression
    consequence: [(identifier) (member_expression)] @_balias_then
    alternative: [(identifier) (member_expression)] @_balias_else)
  (#match? @_balias_then "^(describe|suite|{{block_aliases}})(\\s*\\.\\s*\\w+)?$")
  (#match? @_balias_else "^(describe|suite|{{block_aliases}})(\\s*\\.\\s*\\w+)?$"))

(variable_declarator
  name: (identifier) @test.alias
  value: [(identifier) (member_expression)] @_talias
  (#match? @_talias "^(it|test|{{test_aliases}})(\\s*\\.\\s*(only|skip|fails|todo|fixme|slow|concurrent|sequential))?$"))

(variable_declarator
  name: (identifier) @test.alias
  value: (ternary_expression
    consequence: [(identifier) (member_expression)] @_talias_then
    alternative: [(identifier) (member_expression)] @_talias_else)
  (#match? @_talias_then "^(it|test|{{test_aliases}})(\\s*\\.\\s*(only|skip|fails|todo|fixme|slow|concurrent|sequential))?$")
  (#match? @_talias_else "^(it|test|{{test_aliases}})(\\s*\\.\\s*(only|skip|fails|todo|fixme|slow|concurrent|sequential))?$"))
