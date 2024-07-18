#!/usr/bin/env bash

tree-sitter generate
node scripts/patch-bindings.js
