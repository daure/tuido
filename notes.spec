# Notes Feature Specification

## Card Component Mockup

+------------------------------------------------------------------+
| Title text input                                                 |
+------------------------------------------------------------------+
| Description text area (markdown)                                 |
| ...                                                              |
+------------------------------------+-----------------------------+
| Space dropdown (30%)               | Tags input (70%)            |
+------------------------------------+-----------------------------+
| URL links control list                                           |
| ...                                                              |
+------------------------------------------------------------------+

## Scenarios

### Scenario 1: Create new note
- **Given** user is on Notes tab or presses `Shift+N`
- **When** user enters valid note title and submits
- **Then** new note card is created at top of feed (index 0) and selected

### Scenario 2: Navigate note cards feed
- **Given** user is on Notes tab in list navigation mode
- **When** user presses `j`/`k`, `Ctrl+u`/`Ctrl+d`, `PgUp`/`PgDn`, or `g`/`G`
- **Then** selection moves between note cards and viewport scrolls

### Scenario 3: Inline note edit mode
- **Given** note card is selected in list navigation mode
- **When** user presses `Enter`
- **Then** note card enters inline edit mode (other cards mute)
- **And** user cycles fields with `Tab`/`Shift+Tab` and exits with `Esc` or `Ctrl+[`

### Scenario 4: Live search filtering
- **Given** user is on Notes tab
- **When** user presses `/` and types search query
- **Then** feed filters notes matching title, description, space, tags, or links

### Scenario 5: Delete note
- **Given** note card is selected in list navigation mode
- **When** user presses `Ctrl+X` and confirms deletion
- **Then** note is deleted and removed from feed
