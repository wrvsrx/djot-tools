#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"
#include <stdio.h>

// #define TREE_SITTER_DEBUG

#ifdef TREE_SITTER_DEBUG
#include <assert.h>
#endif

// The different tokens the external scanner support
// See `externals` in `grammar.js` for a description of most of them.
enum TokenType {
  BLOCK_LIKE_START,
  BLOCK_LIKE_END,
  HEADING_MARKER,
  SOFTBREAK,
  IGNORED,
};

enum BlockLikeType {
  PARAGRAPH,
  BLANKLINE,
  HEADING,
};

struct Empty {};

union BlockLikeMetadata {
  struct Empty paragraph;
  struct Empty blankline;
  uint8_t heading;
};

struct BlockLike {
  enum BlockLikeType type;
  union BlockLikeMetadata metadata;
};
typedef Array(struct BlockLike) BlockLikeStack;

// our lexer might emit tokens in sequence
// 1. optional(block_like_start)
// 2. optional(starting_marker)
// 3. optional(ignored after starting_marker)
// 3. remaining
enum LineParsingState {
  NOT_PARSING_BLOCK_LIKE_START,
  NOT_PARSING_STARTING_MARKER,
  NOT_PARSING_IGNORED_AFTER_STARTING_MARKER,
  OTHERWISE,
};

struct ScannerState {
  // have we parse the line start
  // use this flag to avoid stuck at empty line
  // this flag is reset every time we consume '\n'
  enum LineParsingState line_parsing_state;
  // store all open blocks
  BlockLikeStack block_like_stack;
};

void init(struct ScannerState *s) {
  array_init(&(s->block_like_stack));
  s->line_parsing_state = NOT_PARSING_BLOCK_LIKE_START;
}
void *tree_sitter_djot_external_scanner_create(void) {
  struct ScannerState *s = ts_malloc(sizeof(struct ScannerState));
  init(s);
  return s;
}

void tree_sitter_djot_external_scanner_destroy(void *payload) {
  struct ScannerState *s = payload;
  ts_free(s);
}

#define SAVE_TO_BUFFER(buffer, size, value)                                    \
  *(__typeof__(value) *)(buffer + size) = value;                               \
  size += sizeof(__typeof__(value));

#define LOAD_FROM_BUFFER(buffer, size, value)                                  \
  value = *(__typeof__(value) *)(buffer + size);                               \
  size += sizeof(__typeof__(value));

unsigned tree_sitter_djot_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  struct ScannerState *s = payload;
  unsigned size = 0;
  SAVE_TO_BUFFER(buffer, size, s->line_parsing_state);
  SAVE_TO_BUFFER(buffer, size, s->block_like_stack.size);
  for (__typeof__(s->block_like_stack.size) i = 0; i < s->block_like_stack.size;
       ++i) {
    SAVE_TO_BUFFER(buffer, size, s->block_like_stack.contents[i]);
  }
  return size;
}

void tree_sitter_djot_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  struct ScannerState *s = payload;
  init(s);

  if (length == 0) {
    return;
  }

  unsigned size = 0;
  LOAD_FROM_BUFFER(buffer, size, s->line_parsing_state);
  __typeof__(s->block_like_stack.size) count;
  LOAD_FROM_BUFFER(buffer, size, count);

  array_grow_by(&(s->block_like_stack), count);
  for (__typeof__(count) i = 0; i < count; ++i) {
    LOAD_FROM_BUFFER(buffer, size, s->block_like_stack.contents[i]);
  }
  assert(length == size);
}

void consume_whitespace(TSLexer *lexer) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }
}

void push_block_like(struct ScannerState *s, struct BlockLike b) {
#ifdef TREE_SITTER_DEBUG
  if (b.type == PARAGRAPH) {
    printf("--- push paragraph\n");
  } else if (b.type == BLANKLINE) {
    printf("--- push blankline\n");
  } else if (b.type == HEADING) {
    printf("--- push heading %d\n", b.metadata.heading);
  } else {
    assert(false);
  }
#endif
  array_push(&(s->block_like_stack), b);
}

void pop_block_like(struct ScannerState *s) {
  struct BlockLike t = *array_back(&(s->block_like_stack));
#ifdef TREE_SITTER_DEBUG
  if (t.type == PARAGRAPH) {
    printf("---pop paragraph\n");
  } else if (t.type == BLANKLINE) {
    printf("---pop blankline\n");
  }
#endif
  array_pop(&(s->block_like_stack));
}

static void accpet_block_like_end(struct ScannerState *s, TSLexer *lexer,
                                  const bool *valid_symbols) {
  assert(valid_symbols[BLOCK_LIKE_END]);
  lexer->result_symbol = BLOCK_LIKE_END;
  pop_block_like(s);
}

static void accpet_softbreak(struct ScannerState *s, TSLexer *lexer,
                             const bool *valid_symbols) {
#ifdef TREE_SITTER_DEBUG
  printf("--- accept softbreak\n");
#endif
  assert(valid_symbols[SOFTBREAK]);
  lexer->result_symbol = SOFTBREAK;
}

static uint8_t count_heading_level(TSLexer *lexer) {
  uint8_t heading_level = 0;
  while (lexer->lookahead == '#') {
    ++heading_level;
    lexer->advance(lexer, false);
  }
#ifdef TREE_SITTER_DEBUG
  printf("--- heading level %d\n", heading_level);
#endif
  return heading_level;
}

static bool parse_eol(struct ScannerState *s, TSLexer *lexer,
                      const bool *valid_symbols) {
  // if it's eol
  // we must accpet that since we always parse eol manually
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  s->line_parsing_state = NOT_PARSING_BLOCK_LIKE_START;
#ifdef TREE_SITTER_DEBUG
  printf("--- state from OTHERWISE to START_PARSING_IGNORED\n");
#endif
  assert(s->block_like_stack.size > 0);
  struct BlockLike t = *array_back(&(s->block_like_stack));
  if (t.type == BLANKLINE) {
    accpet_block_like_end(s, lexer, valid_symbols);
  } else if (t.type == PARAGRAPH) {
    if (lexer->eof(lexer)) {
      accpet_block_like_end(s, lexer, valid_symbols);
    } else {
      consume_whitespace(lexer);
      bool is_newline = lexer->lookahead == '\n';
      if (is_newline) {
        accpet_block_like_end(s, lexer, valid_symbols);
      } else {
        accpet_softbreak(s, lexer, valid_symbols);
      }
    }
  } else if (t.type == HEADING) {
    if (lexer->eof(lexer)) {
      accpet_block_like_end(s, lexer, valid_symbols);
    } else {
      consume_whitespace(lexer);
      if (lexer->lookahead == '\n') {
        accpet_block_like_end(s, lexer, valid_symbols);
      } else if (lexer->lookahead == '#') {
        uint8_t heading_level = count_heading_level(lexer);
        if (heading_level == t.metadata.heading) {
          accpet_softbreak(s, lexer, valid_symbols);
        } else {
          accpet_block_like_end(s, lexer, valid_symbols);
        }
      } else {
        accpet_softbreak(s, lexer, valid_symbols);
      }
    }
  } else {
    assert(false);
  }
  return true;
}

bool tree_sitter_djot_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  struct ScannerState *s = payload;

  if (lexer->eof(lexer)) {
    // it might not true when we have other kind of blocks
    // it's only true now
    assert(s->block_like_stack.size == 0);
    return false;
  }

  if (s->line_parsing_state == NOT_PARSING_BLOCK_LIKE_START) {
    assert(lexer->get_column(lexer) == 0);
    consume_whitespace(lexer);
    if (s->block_like_stack.size == 0) {
      // if no block is open, search for block start
      if (lexer->lookahead == '\n') {
        struct BlockLike b = {.type = BLANKLINE, .metadata = {.blankline = {}}};
        push_block_like(s, b);
      } else if (lexer->lookahead == '#') {
        // we just count for # number, don't consume them
        lexer->mark_end(lexer);
        uint8_t heading_level = count_heading_level(lexer);
        struct BlockLike b = {.type = HEADING,
                              .metadata = {.heading = heading_level}};
        push_block_like(s, b);
      } else {
        struct BlockLike b = {.type = PARAGRAPH, .metadata = {.paragraph = {}}};
        push_block_like(s, b);
      }
      assert(valid_symbols[BLOCK_LIKE_START]);
      lexer->result_symbol = BLOCK_LIKE_START;
    } else {
      struct BlockLike t = *array_back(&(s->block_like_stack));
      if (t.type == PARAGRAPH) {
        // if current block is paragraph, continue parsing
        assert(valid_symbols[IGNORED]);
        lexer->result_symbol = IGNORED;
      } else if (t.type == BLANKLINE) {
        // if current block is blankline, it's impossible
        assert(false);
      } else if (t.type == HEADING) {
        assert(valid_symbols[IGNORED]);
        lexer->result_symbol = IGNORED;
      } else {
        assert(false);
      }
    }
    s->line_parsing_state = NOT_PARSING_STARTING_MARKER;
    return true;
  } else if (s->line_parsing_state == NOT_PARSING_STARTING_MARKER) {
    assert(s->block_like_stack.size > 0);
    struct BlockLike t = *array_back(&(s->block_like_stack));
    if (t.type == HEADING && lexer->lookahead == '#') {
      // if current block is heading, there is two possible cases
      // 1. it's a heading marker
      // 2. it's simple continuation
      // we just need to handle the first case, the latter can be handled by
      // default scanner
      uint8_t heading_level = count_heading_level(lexer);
      assert(heading_level == t.metadata.heading);
      assert(valid_symbols[HEADING_MARKER]);
      lexer->result_symbol = HEADING_MARKER;
    } else {
      assert(valid_symbols[IGNORED]);
      lexer->result_symbol = IGNORED;
    }
    s->line_parsing_state = NOT_PARSING_IGNORED_AFTER_STARTING_MARKER;
    return true;
  } else if (s->line_parsing_state ==
             NOT_PARSING_IGNORED_AFTER_STARTING_MARKER) {
    consume_whitespace(lexer);
    assert(valid_symbols[IGNORED]);
    lexer->result_symbol = IGNORED;
    s->line_parsing_state = OTHERWISE;
    return true;
  } else if (s->line_parsing_state == OTHERWISE) {
#ifdef TREE_SITTER_DEBUG
    printf("--- counter %c\n", lexer->lookahead);
#endif
    if (lexer->lookahead == '\n') {
      // if it isn't start or start has been parsed
      assert(parse_eol(s, lexer, valid_symbols));
      return true;
    }
  } else {
    assert(false);
  }

  return false;
}
