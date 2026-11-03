# Solaris native client 1.0.0-rc.4 — draft

Changes in the current RC4 candidate since RC3 (not a release approval):

- Updates to entity rigs, animation, equipment and block-entity rendering.
  The cushion and sulfur-cube candidate rigs and their rendering-specific
  facts were dropped because their provenance did not meet the project's
  authoring rule; those entities are not visually accepted in this candidate.
- Graphics preset and rendering changes, including water, lighting and atmospheric effects.
- Client automation, interface and audio changes, plus launcher and packaging updates.

The client targets Minecraft 26.3, protocol 777. Singleplayer uses the launcher's
pinned 26.3 server-content bundle in `servers/content/26.3`.

## Release blockers and evidence

This draft describes the candidate source, not a claim that every change has
passed live acceptance. Final server and plugin workflow pins must be selected
before release. Integrated builds, tests, native-scene checks and platform
packaging still require their respective receipts.
