# Inpainter Strategies

Strategies are the assistant's skills about the work as a whole: how to use several of Inpainter's sub-applications together to solve a kind of problem. Each sub-application's own skills cover only its own work and stay with that sub-application. This repository holds the strategies, apart from Inpainter's code, so they can be refined as often as the work needs.

## Format

A strategy has the same format as a Claude Code skill, so a person can write their own. Each strategy is a folder holding a `SKILL.md`:

```text
<strategy-name>/
  SKILL.md
```

`SKILL.md` begins with a name and a one-line description, then the guidance:

```markdown
---
name: <strategy-name>
description: <what the strategy does and when to use it>
---

# <Title>

<The guidance the assistant follows.>
```

The assistant sees every strategy's name and description, and reads the whole file when one fits the request.
