; Blocks: describe("...") / suite("..."), including describe.only / .skip / .sequential.
(call_expression
  function: [(identifier) @_block_fn
             (member_expression object: (identifier) @_block_fn)]
  arguments: (arguments . (string (string_fragment) @block.name))
  (#match? @_block_fn "^(describe|suite)$")) @block

; Tests: it("...") / test("..."), including it.only / .skip / .fails / .todo.
(call_expression
  function: [(identifier) @_test_fn
             (member_expression object: (identifier) @_test_fn)]
  arguments: (arguments . (string (string_fragment) @test.name))
  (#match? @_test_fn "^(it|test)$")) @test
