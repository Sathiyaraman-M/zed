; Indentation rules for MSBuild / XML (.csproj, .props, .targets)

(STag ">" @end) @indent
(EmptyElemTag "/>" @end) @indent

(element
  (STag) @start
  (ETag)? @end) @indent
