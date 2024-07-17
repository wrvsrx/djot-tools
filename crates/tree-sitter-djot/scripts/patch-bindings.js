const fs = require('fs');
const path = require('path');

{
  const src = `// NOTE: if your language uses an external scanner, uncomment this block:
    /*
    let scanner_path = src_dir.join("scanner.c");
    c_config.file(&scanner_path);
    println!("cargo:rerun-if-changed={}", scanner_path.to_str().unwrap());
    */
`;

  const dst = `let scanner_path = src_dir.join("scanner.c");
    c_config.file(&scanner_path);
    println!("cargo:rerun-if-changed={}", scanner_path.to_str().unwrap());
`;

  const filePath = path.join('bindings', 'rust', 'build.rs');
  fs.readFile(filePath, 'utf8', function (err, data) {
    if (err) {
      return console.log(err);
    }
    let newContent = data.replace(src, dst);

    fs.writeFile(filePath, newContent, 'utf8', function (err) {
      if (err) return console.log(err);
    });
  });
}

{
  const src = `        # NOTE: if your language has an external scanner, add it here.
`;

  const dst = `        # NOTE: if your language has an external scanner, add it here.
        "src/scanner.c",
`;

  const filePath = path.join('binding.gyp');
  fs.readFile(filePath, 'utf8', function (err, data) {
    if (err) {
      return console.log(err);
    }
    let newContent = data.replace(src, dst);

    fs.writeFile(filePath, newContent, 'utf8', function (err) {
      if (err) return console.log(err);
    });
  });
}
