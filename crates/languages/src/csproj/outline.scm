; Outline queries for MSBuild / XML (.csproj, .props, .targets)
; Capture XML elements and self-closing tags so they appear in the outline view.

(element
  (STag
    (Name) @name)) @item

(EmptyElemTag
  (Name) @name) @item

(doctypedecl
  (Name) @name) @item
