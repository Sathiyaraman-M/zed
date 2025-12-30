; Razor-specific highlighting
; This file scopes C# queries to only apply within Razor C# contexts, not HTML
; NO HIDDEN NODES (starting with _) are queried to ensure compatibility with Zed

;; ============================================================================
;; Razor Comments
;; ============================================================================

[
  (razor_comment)
  (html_comment)
] @comment

;; ============================================================================
;; Razor Directives
;; ============================================================================

[
  "at_page"
  "at_using"
  "at_model"
  "at_rendermode"
  "at_inject"
  "at_implements"
  "at_layout"
  "at_inherits"
  "at_attribute"
  "at_typeparam"
  "at_namespace"
  "at_preservewhitespace"
  "at_at_escape"
  "at_colon_transition"
] @keyword.directive

"at_block" @keyword

[
  "at_lock"
  "at_section"
] @keyword

[
  "at_if"
  "at_switch"
] @keyword.conditional

[
  "at_for"
  "at_foreach"
  "at_while"
  "at_do"
] @keyword.repeat

[
  "at_try"
  "catch"
  "finally"
] @keyword.exception

[
  "at_implicit"
  "at_explicit"
] @keyword

"at_await" @keyword.coroutine

(razor_rendermode) @constant.builtin

(razor_attribute_name) @attribute

;; ============================================================================
;; HTML Elements - Basic punctuation only (no hidden nodes)
;; ============================================================================

(element "<" @tag)
(element ">" @tag)
(element "</" @tag)
(element "/>" @tag)

;; ============================================================================
;; C# Code in Razor Blocks
;; ============================================================================

;; Variables
(razor_block (identifier) @variable)
(razor_explicit_expression (parenthesized_expression (identifier) @variable))
(razor_implicit_expression (identifier) @variable)
(razor_await_expression (identifier) @variable)

;; Methods
(razor_block (method_declaration name: (identifier) @function))
(razor_block (local_function_statement name: (identifier) @function))

;; Types
(razor_block (interface_declaration name: (identifier) @type))
(razor_block (class_declaration name: (identifier) @type))
(razor_block (enum_declaration name: (identifier) @type))
(razor_block (struct_declaration (identifier) @type))
(razor_block (record_declaration (identifier) @type))
(razor_block (namespace_declaration name: (identifier) @module))
(razor_block (generic_name (identifier) @type))
(razor_explicit_expression (parenthesized_expression (generic_name (identifier) @type)))
(razor_implicit_expression (generic_name (identifier) @type))
(razor_block (type_parameter (identifier) @type.parameter))
(razor_block (parameter type: (identifier) @type))
(razor_block (parameter name: (identifier) @variable.parameter))
(razor_block (type_argument_list (identifier) @type))
(razor_block (as_expression right: (identifier) @type))
(razor_block (is_expression right: (identifier) @type))
(razor_block (constructor_declaration name: (identifier) @constructor))
(razor_block (destructor_declaration name: (identifier) @constructor))
(razor_block (base_list (identifier) @type))
(razor_block (predefined_type) @type.builtin)
(razor_explicit_expression (parenthesized_expression (predefined_type) @type.builtin))
(razor_implicit_expression (predefined_type) @type.builtin)
(razor_block (enum_member_declaration (identifier) @constant))

;; Literals
(razor_block [(real_literal) (integer_literal)] @number)
(razor_explicit_expression (parenthesized_expression [(real_literal) (integer_literal)] @number))
(razor_implicit_expression [(real_literal) (integer_literal)] @number)

(razor_block [(character_literal) (string_literal) (raw_string_literal) (verbatim_string_literal) (interpolated_string_expression)] @string)
(razor_explicit_expression (parenthesized_expression [(character_literal) (string_literal) (raw_string_literal) (verbatim_string_literal) (interpolated_string_expression)] @string))
(razor_implicit_expression [(character_literal) (string_literal) (raw_string_literal) (verbatim_string_literal) (interpolated_string_expression)] @string)

(razor_block [(interpolation_start) (interpolation_quote)] @punctuation.special)
(razor_block (escape_sequence) @string.escape)
(razor_explicit_expression (parenthesized_expression (escape_sequence) @string.escape))
(razor_implicit_expression (escape_sequence) @string.escape)

(razor_block [(boolean_literal) (null_literal)] @constant.builtin)
(razor_explicit_expression (parenthesized_expression [(boolean_literal) (null_literal)] @constant.builtin))
(razor_implicit_expression [(boolean_literal) (null_literal)] @constant.builtin)

;; Comments
(razor_block (comment) @comment)

;; Punctuation
(razor_block [";" "." ","] @punctuation.delimiter)
(razor_explicit_expression [";" "." ","] @punctuation.delimiter)
(razor_implicit_expression ["." ","] @punctuation.delimiter)

;; Operators
(razor_block ["--" "-" "-=" "&" "&=" "&&" "+" "++" "+=" "<" "<=" "<<" "<<=" "=" "==" "!" "!=" "=>" ">" ">=" ">>" ">>=" ">>>" ">>>=" "|" "|=" "||" "?" "??" "??=" "^" "^=" "~" "*" "*=" "/" "/=" "%" "%=" ":"] @operator)
(razor_explicit_expression ["--" "-" "-=" "&" "&=" "&&" "+" "++" "+=" "<" "<=" "<<" "<<=" "=" "==" "!" "!=" "=>" ">" ">=" ">>" ">>=" ">>>" ">>>=" "|" "|=" "||" "?" "??" "??=" "^" "^=" "~" "*" "*=" "/" "/=" "%" "%=" ":"] @operator)
(razor_implicit_expression ["--" "-" "&" "&=" "&&" "+" "++" "+=" "!" "!=" "=>" "|" "|=" "||" "?" "??" "^" "^=" "~" "*" "/" "%" "."] @operator)

;; Brackets
(razor_block ["(" ")" "[" "]" "{" "}" (interpolation_brace)] @punctuation.bracket)
(razor_explicit_expression ["(" ")" "[" "]" "{" "}" (interpolation_brace)] @punctuation.bracket)
(razor_implicit_expression ["(" ")" "[" "]" "{" "}" (interpolation_brace)] @punctuation.bracket)

;; Keywords
(razor_block [(modifier) "this" (implicit_type)] @keyword)
(razor_explicit_expression [(modifier) "this"] @keyword)
(razor_implicit_expression [(modifier) "this"] @keyword)

(razor_block ["add" "alias" "as" "base" "break" "case" "catch" "checked" "class" "continue" "default" "delegate" "do" "else" "enum" "event" "explicit" "extern" "finally" "for" "foreach" "global" "goto" "if" "implicit" "interface" "is" "lock" "namespace" "notnull" "operator" "params" "return" "remove" "sizeof" "stackalloc" "static" "struct" "switch" "throw" "try" "typeof" "unchecked" "using" "while" "new" "await" "in" "yield" "get" "set" "when" "out" "ref" "from" "where" "select" "record" "init" "with" "let"] @keyword)

(razor_explicit_expression ["as" "is" "new" "await" "typeof" "sizeof" "checked" "unchecked" "default"] @keyword)
(razor_implicit_expression ["new" "await"] @keyword)

;; Attributes
(razor_block (attribute name: (identifier) @attribute))

;; Method calls
(razor_block (invocation_expression (member_access_expression name: (identifier) @function)))
(razor_explicit_expression (parenthesized_expression (invocation_expression (member_access_expression name: (identifier) @function))))
(razor_implicit_expression (invocation_expression (member_access_expression name: (identifier) @function)))
(razor_block (invocation_expression (identifier) @function))
(razor_explicit_expression (parenthesized_expression (invocation_expression (identifier) @function)))
(razor_implicit_expression (invocation_expression (identifier) @function))

;; Type constraints
(razor_block (type_parameter_constraints_clause (identifier) @type))

;; Control flow
(razor_if (razor_condition [(real_literal) (integer_literal)] @number))
(razor_if (razor_condition [(boolean_literal) (null_literal)] @constant.builtin))
(razor_if (razor_condition (identifier) @variable))
(razor_for [(real_literal) (integer_literal)] @number)
(razor_for (identifier) @variable)
(razor_foreach (identifier) @variable)
(razor_while (razor_condition (identifier) @variable))
(razor_switch (razor_condition (identifier) @variable))

;; Directive contexts - using only exposed nodes
(razor_using_directive (identifier) @type)
(razor_using_directive (qualified_name) @module)
(razor_inject_directive (variable_declaration (identifier) @type))
(razor_inject_directive (variable_declaration (variable_declarator (identifier) @variable)))
(razor_namespace_directive (qualified_name) @module)
(razor_attribute_directive (attribute_list (attribute (identifier) @attribute)))
