---
name: tb-backlog
description: Do the next backlog tasks for a given project
---

Review the tasks currently in taskboard's backlog.

If there are tasks for multiple projects, we'll need the user's input on which project to
execute for unless they've already told us.


## If there's an Epic in the backlog:

If there are multiple epics, clear up with the user which epic they'd like to pursue, if
they haven't already told us.

For the epic, look first and see if there are any tasks related to the epic already in the
backlog. If so, pursue those.

If there are tasks unrelated to the epic in the backlog, we won't pursue those.

If there are no tasks related to the epic in the backlog, take a look at the epic's current
status - see how the work is broken down, and then look and see which should be done next.

We most likely don't want to do the whole epic at once, unless the user told us to. We'll
simply look for tasks for the next section or phase of work and do those, even if it's only
one task.

We'll work on the selected tasks using the instructions for handling multiple tasks.



## If there are one or more individual tasks in the backlog:

Iterate over the backlog tasks in taskboard, using the instructions for handling multiple tasks.


## Handling multiple tasks

Consider them as a whole: can they be done in parallel, or do they need to be done synchronously?
Assign them to subagents as appropriate to perform each task: do not do task work in the main agent.
Agents should perform work in a branch within a worktree. When work is done (including testing),
it should be committed to the working branch, and then reviewed. Once its been reviewed and any necessary
changes have been added, tested, and committed, the branch should be merged to main and the old worktree
should be removed. Only return flow to the user when a question is necessary to clear up requirements.

