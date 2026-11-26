# Dosa

## Before opening a pull request

Use [$verify-dosa](.agents/skills/verify-dosa/SKILL.md) on the revision the PR will open with. Run its baseline and the feature recipes that cover the changed user paths. If a later edit changes the behavior you verified, rerun the affected check before opening the PR.

Include `## Verification` in the PR description, after `## Blast Radius` when present. Use one to three bullets. Name the command or user action and the observed outcome. Report failed or blocked checks with the attempted path and the reason. Link reviewer-accessible evidence when available.
