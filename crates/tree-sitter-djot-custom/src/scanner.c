#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"
#include <stdint.h>

// #define TREE_SITTER_DEBUG

#ifdef TREE_SITTER_DEBUG
#include <assert.h>
#include <stdio.h>
#endif

// The different tokens the external scanner support
// See `externals` in `grammar.js` for a description of most of them.
enum TokenType {
  SECTION_START,
  SECTION_END,

  HEADING_START,
  HEADING_MARKER,
  HEADING_END,

  PARAGRAPH_START,
  PARAGRAPH_END,

  BLANKLINE_START,
  BLANKLINE_END,

  STR,
  SOFTBREAK,

  IGNORED,
};

enum BlockType {
  DOCUMENT,
  SECTION,
  PARAGRAPH,
  HEADING,
  BLANKLINE,
};

struct Empty {};

struct HeadingState {
  uint8_t level;
};

struct SectionState {
  uint8_t level;
};

union BlockData {
  struct Empty document;
  struct SectionState section;
  struct HeadingState heading;
  struct Empty paragraph;
  struct Empty blankline;
};

struct Block {
  enum BlockType type;
  union BlockData data;
};
typedef Array(struct Block) BlockArray;
static char const *block_name(enum BlockType type) {
  switch (type) {
  case DOCUMENT:
    return "DOCUMENT";
  case SECTION:
    return "SECTION";
  case PARAGRAPH:
    return "PARAGRAPH";
  case HEADING:
    return "HEADING";
  case BLANKLINE:
    return "BLANKLINE";
  }
}

struct Token {
  enum TokenType type;
  uint32_t length;
};
typedef Array(struct Token) TokenArray;
static char const *token_name(enum TokenType type) {
  switch (type) {
  case SECTION_START:
    return "SECTION_START";
  case SECTION_END:
    return "SECTION_END";
  case HEADING_START:
    return "HEADING_START";
  case HEADING_MARKER:
    return "HEADING_MARKER";
  case HEADING_END:
    return "HEADING_END";
  case PARAGRAPH_START:
    return "PARAGRAPH_START";
  case PARAGRAPH_END:
    return "PARAGRAPH_END";
  case BLANKLINE_START:
    return "BLANKLINE_START";
  case BLANKLINE_END:
    return "BLANKLINE_END";
  case STR:
    return "STR";
  case SOFTBREAK:
    return "SOFTBREAK";
  case IGNORED:
    return "IGNORED";
  }
}

enum LineParserState {
  PARSING_BLOCK_START,
  PARSING_INLINE,
  PARSING_EOL,
};

struct ScannerState {
  enum LineParserState line_parsing_state;
  // store all open blocks
  BlockArray block_array;
  TokenArray remained_tokens;
};

// ---- function declaration start ----
static void initState(struct ScannerState *s);

static bool followedByWhitespace(TSLexer *lexer);
static bool followedByEol(TSLexer *lexer);

static void pop_token(struct ScannerState *s);

static void tryContainersStarts(struct ScannerState *s, TSLexer *lexer,
                                const bool *valid_symbols);
static void try_parse_eol(struct ScannerState *s, TSLexer *lexer,
                          const bool *valid_symbols);
static void try_parse_inline(struct ScannerState *s, TSLexer *lexer,
                             const bool *valid_symbols);

static enum TokenType close_block_token_type(enum BlockType const t);
static void close_block(struct ScannerState *s, TSLexer *lexer,
                        const bool *valid_symbols, uint32_t const length);
static void try_closing_blocks_when_meeting_blankline(struct ScannerState *s,
                                                      TSLexer *lexer,
                                                      const bool *valid_symbols,
                                                      uint32_t length);
static void try_closing_blocks_when_meeting_possible_heading(
    struct ScannerState *s, TSLexer *lexer, const bool *valid_symbols,
    uint32_t length);
// ---- function declaration end ----

void *tree_sitter_djot_external_scanner_create(void) {
  struct ScannerState *s = ts_malloc(sizeof(struct ScannerState));
  initState(s);
  {
    struct Block const b = {.type = DOCUMENT, .data = {.document = {}}};
    array_push(&(s->block_array), b);
  }
  return s;
}

void tree_sitter_djot_external_scanner_destroy(void *payload) {
  struct ScannerState *s = payload;
  array_delete(&(s->block_array));
  array_delete(&(s->remained_tokens));
  ts_free(s);
}

#define FOR(index, upper_bound)                                                \
  for (__typeof__(upper_bound) index = 0; index < upper_bound; ++index)

#define SAVE_TO_BUFFER(buffer, size, value)                                    \
  *(__typeof__(value) *)(buffer + size) = value;                               \
  size += sizeof(__typeof__(value));

#define SAVE_ARRAY(buffer, size, array)                                        \
  SAVE_TO_BUFFER(buffer, size, array->size);                                   \
  FOR(i, array->size) { SAVE_TO_BUFFER(buffer, size, array->contents[i]) }

#define LOAD_FROM_BUFFER(buffer, size, value)                                  \
  value = *(__typeof__(value) *)(buffer + size);                               \
  size += sizeof(__typeof__(value));

#define LOAD_ARRAY(buffer, size, array)                                        \
  {                                                                            \
    uint32_t count = 0;                                                        \
    LOAD_FROM_BUFFER(buffer, size, count);                                     \
    array_grow_by(array, count);                                               \
    FOR(i, array->size) { LOAD_FROM_BUFFER(buffer, size, array->contents[i]) } \
  }

unsigned tree_sitter_djot_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  struct ScannerState *s = payload;
  unsigned size = 0;
  SAVE_TO_BUFFER(buffer, size, s->line_parsing_state);
  SAVE_ARRAY(buffer, size, (&(s->block_array)));
  SAVE_ARRAY(buffer, size, (&(s->remained_tokens)));
  return size;
}

void tree_sitter_djot_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  struct ScannerState *s = payload;
  initState(s);

  if (length == 0) {
    {
      struct Block const b = {.type = DOCUMENT, .data = {.document = {}}};
      array_push(&(s->block_array), b);
    }
    return;
  }

  unsigned size = 0;
  LOAD_FROM_BUFFER(buffer, size, (s->line_parsing_state));
  LOAD_ARRAY(buffer, size, (&(s->block_array)))
  LOAD_ARRAY(buffer, size, (&(s->remained_tokens)))
#ifdef TREE_SITTER_DEBUG
  if (size != length) {
    printf("---- deserialize size: %d\n", length);
    printf("---- actual size: %d\n", size);
  }
#endif
  assert(length == size);
}

bool tree_sitter_djot_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  struct ScannerState *s = payload;

  // pop previous stored token out
  if (s->remained_tokens.size > 0) {
    struct Token *t = array_back(&(s->remained_tokens));
#ifdef TREE_SITTER_DEBUG
    printf("---- pop token %s\n", token_name(t->type));
#endif
    assert(valid_symbols[t->type]);
    lexer->result_symbol = t->type;
    FOR(i, t->length) { lexer->advance(lexer, false); }
    lexer->mark_end(lexer);
    array_pop(&(s->remained_tokens));
    return true;
  }

  assert(s->remained_tokens.size == 0);

  // deal with eof
  if (lexer->eof(lexer)) {
    return false;
  }

  // if we're parsing block start, column should be 0
  if (s->line_parsing_state == PARSING_BLOCK_START) {
    assert(lexer->get_column(lexer) == 0);
  }

  // we must emit token to change state
  // so we emit ignored if we don't emit any token
  // therefore we can't call mark_end after that
  // we can only push token to remained_tokens
  {
    while (followedByWhitespace(lexer)) {
      lexer->advance(lexer, true);
    }
    lexer->mark_end(lexer);
    lexer->result_symbol = IGNORED;
  }

  // there're three possible state
  // 1. PARSING_BLOCK_START
  // 2. PARSING_INLINE
  // 3. PARSING_EOL
  //
  // when we at line start, there's two possible state
  // if it's first line or previous eol is softbreak, then it's PARSING_INLINE
  // otherwise, it's PARSING_BLOCK_START

#ifdef TREE_SITTER_DEBUG
  printf("---- current column %d\n", lexer->get_column(lexer));
  switch (s->line_parsing_state) {
  case PARSING_BLOCK_START:
    printf("---- line parsing state PARSING_BLOCK_START\n");
    break;
  case PARSING_INLINE:
    printf("---- line parsing state PARSING_INLINE\n");
    break;
  case PARSING_EOL:
    printf("---- line parsing state PARSING_EOL\n");
    break;
  }
#endif
  // since we parse all token by external scanner
  // all parse result should be true
  switch (s->line_parsing_state) {
  case PARSING_BLOCK_START:
    // parse at line start
    // must start some blocks
    tryContainersStarts(s, lexer, valid_symbols);
    // if parse succecfully, we should have some tokens in remained_tokens
    assert(s->remained_tokens.size > 0);
    break;
  case PARSING_INLINE:
    // parse inline
    // when we account '\n', end it
    try_parse_inline(s, lexer, valid_symbols);
    break;
  case PARSING_EOL:
    // parse eol
    // might end some blocks
    // this parse must success
    assert(followedByEol(lexer));
    try_parse_eol(s, lexer, valid_symbols);
    break;
  }

  // if parse fail and there's nothing in remained_tokens
  // add an empty token to ensure we can proceed
  if (s->remained_tokens.size == 0) {
    array_push(&(s->remained_tokens), (struct Token){.type = IGNORED});
  }

  // reverse remained_tokens
  FOR(i, (s->remained_tokens.size / 2)) {
    __typeof__(s->remained_tokens.contents[0]) tmp =
        *array_get(&(s->remained_tokens), i);
    *array_get(&(s->remained_tokens), i) =
        *array_get(&(s->remained_tokens), s->remained_tokens.size - i - 1);
    *array_get(&(s->remained_tokens), s->remained_tokens.size - i - 1) = tmp;
  }

  return true;
}

// ---- block push and pop start ----
static void push_block(struct ScannerState *s, struct Block b) {
#ifdef TREE_SITTER_DEBUG
  printf("--- push block %s\n", block_name(b.type));
#endif
  array_push(&(s->block_array), b);
}

static void pop_block(struct ScannerState *s) {
  struct Block t = *array_back(&(s->block_array));
#ifdef TREE_SITTER_DEBUG
  printf("--- pop block %s\n", block_name(t.type));
#endif
  array_pop(&(s->block_array));
}
// ---- block push and pop end ----
// ---- token push and pop start ----
static void push_token(struct ScannerState *s, struct Token b) {
#ifdef TREE_SITTER_DEBUG
  printf("--- push token %s\n", token_name(b.type));
#endif
  array_push(&(s->remained_tokens), b);
}

static void pop_token(struct ScannerState *s) {
  struct Token *t = array_back(&(s->remained_tokens));
#ifdef TREE_SITTER_DEBUG
  printf("--- pop token %s\n", token_name(t->type));
#endif
  array_pop(&(s->remained_tokens));
}
// ---- token push and pop end ----

// ---- parser utils start ----
static bool followedByWhitespace(TSLexer *lexer) {
  return lexer->lookahead == ' ' || lexer->lookahead == '\t';
}

static bool followedByEol(TSLexer *lexer) {
  return lexer->lookahead == '\r' || lexer->lookahead == '\n';
}

static bool followedByWhitespaceOrEol(TSLexer *lexer) {
  return followedByWhitespace(lexer) || followedByEol(lexer);
}

static uint32_t try_consume_whitespace(struct ScannerState *s, TSLexer *lexer) {
  uint32_t res = 0;
  while (followedByWhitespace(lexer)) {
    ++res;
    lexer->advance(lexer, true);
  }
  if (res > 0) {
    push_token(s, (struct Token){.type = IGNORED, .length = res});
  }
  return res;
}
// ---- parser utils end ----

// ---- block start ----
static uint8_t count_heading_level(TSLexer *lexer) {
  uint8_t heading_level = 0;
  while (lexer->lookahead == '#') {
    ++heading_level;
    lexer->advance(lexer, false);
  }
  if (!followedByWhitespaceOrEol(lexer)) {
    heading_level = 0;
  }
#ifdef TREE_SITTER_DEBUG
  printf("--- heading level %d\n", heading_level);
#endif
  return heading_level;
}

// parse heading start
static bool try_parse_heading_start(struct ScannerState *s, TSLexer *lexer,
                                    const bool *valid_symbols) {

  struct Block *t = array_back(&(s->block_array));
  uint8_t heading_level = count_heading_level(lexer);
  if (heading_level > 0) {
    switch (t->type) {
    case SECTION:
      // nested section level should be larger than current section
      assert(t->data.section.level < heading_level);
    case DOCUMENT:
      // if we're at top level or other sections, then we should start a section
      // and a heading
      push_block(s,
                 (struct Block){.type = SECTION,
                                .data = {.heading = {.level = heading_level}}});
      push_token(s, (struct Token){.type = SECTION_START, .length = 0});

      // otherwise, we should only start a heading
      push_block(s,
                 (struct Block){.type = HEADING,
                                .data = {.heading = {.level = heading_level}}});
      push_token(s, (struct Token){.type = HEADING_START, .length = 0});
      push_token(
          s, (struct Token){.type = HEADING_MARKER, .length = heading_level});
      break;
    case HEADING:
    case PARAGRAPH:
    case BLANKLINE:
      assert(false);
    }
  } else {
    // it's not a heading
    push_block(s, (struct Block){.type = PARAGRAPH});
    push_token(s, (struct Token){.type = PARAGRAPH_START, .length = 0});
  }
  return true;
}

static bool containOtherBlock(enum BlockType t) {
  switch (t) {
  case DOCUMENT:
  case SECTION:
    return true;
  case PARAGRAPH:
  case HEADING:
  case BLANKLINE:
    return false;
  }
}

// only should be called when previous line doesn't end by softbreak
// or called by itself
// so we must start some block
static void tryContainersStarts(struct ScannerState *s, TSLexer *lexer,
                                const bool *valid_symbols) {
  try_consume_whitespace(s, lexer);

  struct Block const *t = array_back(&(s->block_array));
  bool parse_result = false;
  if (containOtherBlock(t->type)) {
    // they can contain other blocks
    switch (lexer->lookahead) {
    case '#':
      parse_result = try_parse_heading_start(s, lexer, valid_symbols);
      break;
    case '\r':
    case '\n':
      push_block(s, (struct Block){.type = BLANKLINE});
      push_token(s, (struct Token){.type = BLANKLINE_START, .length = 0});
      parse_result = true;
      break;
    default:
      // we start a paragraph by default
      push_block(s, (struct Block){.type = PARAGRAPH});
      push_token(s, (struct Token){.type = PARAGRAPH_START, .length = 0});
      parse_result = true;
      break;
    }
  } else {
    // they can't contain other blocks
    // we should not call this function when previous line ends with softbreak
    // so these case should not happen
    assert(false);
  }

  // parsing must be successful
  assert(parse_result);

  {
    // parse block start recursively
    struct Block const *t = array_back(&(s->block_array));
    if (containOtherBlock(t->type)) {
      tryContainersStarts(s, lexer, valid_symbols);
    }
  }

  // after parse block start, we should start parsing inline
  s->line_parsing_state = PARSING_INLINE;
}

static void try_parse_inline(struct ScannerState *s, TSLexer *lexer,
                             const bool *valid_symbols) {
  if (followedByEol(lexer)) {
    s->line_parsing_state = PARSING_EOL;
    return;
  }
  //  currently we only support STR as inline
  try_consume_whitespace(s, lexer);
  uint32_t length = 0;
  while (!followedByEol(lexer)) {
    lexer->advance(lexer, false);
    ++length;
  }
  push_token(s, (struct Token){.type = STR, .length = length});
  s->line_parsing_state = PARSING_EOL;
}

static void try_parse_eol(struct ScannerState *s, TSLexer *lexer,
                          const bool *valid_symbols) {
  assert(followedByEol(lexer));
  // close some blocks or return softbreak
  // if close some blocks, then change state to PARSING_BLOCK_START
  // otherwise, change state to PARSING_INLINE

  // we can't compute length using while loop
  // since we might meet multiple blankline
  uint32_t length = 0;
  if (lexer->lookahead == '\r') {
    lexer->advance(lexer, false);
    ++length;
  }
  if (lexer->lookahead == '\n') {
    lexer->advance(lexer, false);
    ++length;
  }
#ifdef TREE_SITTER_DEBUG
  printf("---- eol length %d\n", length);
#endif

  // close all blocks while meeting eof
  if (lexer->eof(lexer)) {
#ifdef TREE_SITTER_DEBUG
    printf("---- meet eof\n");
#endif
    while (s->block_array.size > 0) {
      close_block(s, lexer, valid_symbols, length);
      length = 0;
    }
    return;
  }

  // close other blocks depends on nextline
  while (followedByWhitespace(lexer)) {
    lexer->advance(lexer, true);
  }

  switch (lexer->lookahead) {
  case '\r':
  case '\n':
    // nextline is blankline
    try_closing_blocks_when_meeting_blankline(s, lexer, valid_symbols, length);
    break;
  case '#':
    // nextline might be heading
    try_closing_blocks_when_meeting_possible_heading(s, lexer, valid_symbols,
                                                     length);
    break;
  default: {
    struct Block const *t = array_back(&(s->block_array));
    if (t->type == BLANKLINE) {
      // always close blankline
      close_block(s, lexer, valid_symbols, length);
      s->line_parsing_state = PARSING_BLOCK_START;
      break;
    } else {
      // continue previous block
      push_token(s, (struct Token){.type = SOFTBREAK, .length = length});
      s->line_parsing_state = PARSING_INLINE;
      break;
    }
  }
  }
}
// ---- block end ----

// ---- tree-sitter state utils start ----
static void initState(struct ScannerState *s) {
  s->line_parsing_state = PARSING_BLOCK_START;
  array_init(&(s->block_array));
  array_init(&(s->remained_tokens));
}
// ---- tree-sitter state utils end ----

// ---- close block start ----
static enum TokenType close_block_token_type(enum BlockType const t) {
  switch (t) {
  case DOCUMENT:
    assert(false);
  case SECTION:
    return SECTION_END;
  case PARAGRAPH:
    return PARAGRAPH_END;
  case HEADING:
    return HEADING_END;
  case BLANKLINE:
    return BLANKLINE_END;
  }
}
static void close_block(struct ScannerState *s, TSLexer *lexer,
                        const bool *valid_symbols, uint32_t const length) {
  struct Block const *t = array_back(&(s->block_array));
  if (t->type != DOCUMENT) {
    enum TokenType const token_type = close_block_token_type(t->type);
    push_token(s, (struct Token){.type = token_type, .length = length});
  }
  pop_block(s);
}
static void try_closing_blocks_when_meeting_blankline(struct ScannerState *s,
                                                      TSLexer *lexer,
                                                      const bool *valid_symbols,
                                                      uint32_t length) {
  struct Block const *t = array_back(&(s->block_array));
  // when meet blankline, we don't need to close blocks recursively
  // blankline will only close heading, paragraph and blankline
  switch (t->type) {
  case HEADING:
  case PARAGRAPH:
  case BLANKLINE:
    close_block(s, lexer, valid_symbols, length);
    s->line_parsing_state = PARSING_BLOCK_START;
    break;
  case DOCUMENT:
  case SECTION:
    push_token(s, (struct Token){.type = SOFTBREAK, .length = length});
    s->line_parsing_state = PARSING_INLINE;
    break;
  }
}
static void try_closing_blocks_when_meeting_possible_heading(
    struct ScannerState *s, TSLexer *lexer, const bool *valid_symbols,
    uint32_t length) {
#ifdef TREE_SITTER_DEBUG
  printf("---- try_closing_blocks_when_meeting_possible_heading\n");
#endif
  uint8_t heading_level = count_heading_level(lexer);
  if (heading_level > 0) {
    uint32_t break_at = s->block_array.size;
    // when meeting heading, we might stop a section or a heading
    FOR(i, s->block_array.size) {
      __typeof__(s->block_array.size) block_index = s->block_array.size - i - 1;
      struct Block const *t = array_get(&(s->block_array), block_index);
      bool cont = true;
      switch (t->type) {
      case HEADING:
        if (t->data.heading.level != heading_level) {
          break_at = block_index;
        }
        cont = false;
        break;
      case SECTION:
        if (t->data.section.level >= heading_level) {
          break_at = block_index;
        }
        break;
      case BLANKLINE:
        break_at = block_index;
        break;
      case DOCUMENT:
      case PARAGRAPH:
        break;
      }
      if (!cont) {
        break;
      }
    }
#ifdef TREE_SITTER_DEBUG
    printf("---- stack depth: %d, break at: %d\n", s->block_array.size,
           break_at);
#endif
    if (break_at < s->block_array.size) {
      while (s->block_array.size > break_at) {
#ifdef TREE_SITTER_DEBUG
        printf("---- closing block because of heading\n");
#endif
        close_block(s, lexer, valid_symbols, length);
        // only first block end can have non-zero length
        length = 0;
      }
      s->line_parsing_state = PARSING_BLOCK_START;
    } else {
      struct Block const *t = array_back(&(s->block_array));
      if (t->type == HEADING) {
        // we need to continue the heading
        assert(t->data.heading.level == heading_level);
        push_token(s, (struct Token){.type = SOFTBREAK, .length = length});
        push_token(
            s, (struct Token){.type = HEADING_MARKER, .length = heading_level});
        s->line_parsing_state = PARSING_INLINE;
      }
    }
  } else {
    push_token(s, (struct Token){.type = SOFTBREAK, .length = length});
    s->line_parsing_state = PARSING_INLINE;
  }
}
// ---- close block end ----
