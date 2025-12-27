; Bracket pairs for MSBuild / XML (.csproj, .props, .targets)

("<" @open "/>" @close)
("</" @open ">" @close)
("<" @open ">" @close)
(("\"" @open "\"" @close) (#set! rainbow.exclude))
((element (STag) @open (ETag) @close) (#set! newline.only) (#set! rainbow.exclude))
