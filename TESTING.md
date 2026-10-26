# Testing Requirements

- Testability should be built into the program from the ground up: UI is difficult to test, so ensure that functionality is built into lower level functions that can be well tested
- Unit tests should be built into the code for all important functionality
- Tests should be kept minimal: include one common "expected" use case, then an edge case if there's an obvious race condition a user could hit
