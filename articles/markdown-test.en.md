---
title: Markdown Features Test
subtitle: Testing kramdown parser with tables and advanced features
author_name: Test Author
author_email: test@example.com
meta_description: Test article for kramdown markdown parser
published: true
paywall: true
---

# Markdown Feature Test

This article tests various **Markdown features** to ensure our kramdown parser works correctly.

## Headers Work

Different levels of headers should render properly.

### Third Level Header
#### Fourth Level Header
##### Fifth Level Header

## Text Formatting

- **Bold text** works
- *Italic text* works
- ~~Strikethrough text~~ works
- `inline code` works
- [Links to Google](https://google.com) work

## Lists

### Unordered Lists
- Item 1
- Item 2
  - Nested item 2.1
  - Nested item 2.2
- Item 3

### Ordered Lists
1. First item
2. Second item
   1. Nested numbered item
   2. Another nested item
3. Third item

## Code Blocks

```javascript
function hello() {
    console.log("Hello World!");
    return true;
}
```

```ruby
class User < ApplicationRecord
  has_many :articles
  
  def full_name
    "#{first_name} #{last_name}"
  end
end
```

## Tables

This is the big test - tables should now work properly:

| Feature | Status | Notes |
|---------|--------|-------|
| Headers | ✅ Working | All levels supported |
| Tables | ✅ Working | With proper alignment |
| Code blocks | ✅ Working | Syntax highlighting ready |
| Lists | ✅ Working | Nested lists supported |

### Table with Alignment

| Left Aligned | Center Aligned | Right Aligned |
|:-------------|:--------------:|--------------:|
| Left         | Center         | Right         |
| Data         | More Data      | Even More     |

<!-- PAYWALL -->

## Premium Content

This content should only be visible to premium subscribers.

### Advanced Table Features

| Exchange | Fee Structure | API Limits | Support Level |
|----------|---------------|------------|---------------|
| Binance | 0.1% / 0.075% | 1200/min | ⭐⭐⭐⭐⭐ |
| Coinbase Pro | 0.5% / 0.5% | 10/sec | ⭐⭐⭐⭐ |
| Kraken | 0.26% / 0.16% | 1/sec | ⭐⭐⭐ |

## Blockquotes

> This is a blockquote
> that spans multiple lines
> and should render properly.

## Horizontal Rules

---

That's the test! All features should now work properly with kramdown. 