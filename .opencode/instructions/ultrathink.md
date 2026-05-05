# ULTRATHINK Protocol

## TRIGGER

User explicitly requests: **"ULTRATHINK"**

## ACTIVATION

When triggered, immediately suspend brevity rules and engage maximum depth.

## ANALYSIS DIMENSIONS

### 1. Technical Lens

- Architecture implications and trade-offs
- Performance considerations (complexity, memory, I/O)
- Security vulnerabilities and attack vectors
- Scalability and maintainability
- Edge cases and failure modes

### 2. User Experience Lens

- User intent vs explicit request
- Cognitive load and usability
- Accessibility requirements (WCAG if UI)
- Error handling and user feedback

### 3. Project Context Lens

- Impact on existing codebase
- Consistency with current patterns
- Migration or refactoring needs
- Dependencies and integration points

### 4. Future-Proofing Lens

- Long-term maintenance burden
- Extensibility and flexibility
- Technical debt implications
- Documentation needs

## OUTPUT FORMAT (ULTRATHINK MODE)

If the change is trivial, keep the response to 2-3 short paragraphs and skip the full template.

### 1. Deep Reasoning Chain

Detailed breakdown of the problem and solution approach:

- What is the core problem?
- What constraints exist?
- What approaches were considered?
- Why was this approach chosen?

### 2. Edge Case Analysis

- What could go wrong?
- How does the solution handle failures?
- What are the boundary conditions?

### 3. Implementation

Production-ready, well-documented code/solution that:

- Addresses all analyzed dimensions
- Handles edge cases gracefully
- Follows project conventions
- Is optimized for the specific context

### 4. Trade-offs Summary

Brief list of what was optimized for and what was deprioritized

## EXAMPLE TRIGGER

**User:** "ULTRATHINK - implement authentication for my API"

**Response:**

```
## Deep Reasoning Chain
[Detailed analysis of auth requirements, token strategies,
security considerations, session management, etc.]

## Edge Case Analysis
[Token expiration, concurrent sessions, brute force protection,
password reset flows, etc.]

## Implementation
[Complete, production-ready auth solution]

## Trade-offs
- Optimized for: Security, scalability
- Deprioritized: Simplicity (due to security requirements)
```
