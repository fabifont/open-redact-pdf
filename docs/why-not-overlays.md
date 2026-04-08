# Why This Is Not a Canvas Overlay Tool

Drawing a black rectangle over a PDF does not remove the original data. Text can remain searchable, selectable, copyable, or extractable from the file. Flattening entire pages into images avoids that specific leak but destroys useful text structure and is not the design goal of this project.

Open Redact PDF instead works against PDF structure. The engine removes or neutralizes targeted content in the page model, preserves unredacted text where supported, and only adds visible fill marks after the underlying targeted content is no longer kept in the rewritten output.

