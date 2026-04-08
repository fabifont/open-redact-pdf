# Security Model

## What counts as redaction

For this project, redaction means the output PDF no longer contains the redacted text or content in referenced page content that remains accessible after save. A visible overlay is only valid after the underlying targeted content has been removed or neutralized.

## Current guarantees

- Intersecting text glyphs are removed from rewritten text-showing operators.
- Intersecting vector paint operations are neutralized.
- Intersecting image draws are removed conservatively at the image invocation level.
- Optional intersecting annotations can be removed from touched pages.

## Current limitations

- The MVP fails on unsupported content such as Form XObjects or unsupported font types on targeted pages.
- Image redaction is conservative and removes whole image draws when they intersect a target.
- Metadata and attachment stripping are opt-in and limited to supported object layouts.

