// This package needs to be install:
//
// ```
// npm install browser-ui-test
// ```

const path = require("path");
const spawn = require("child_process").spawn;

async function main(argv) {
    let server = "http://127.0.0.1:3000";
    if (typeof process.env.SERVER_URL !== "undefined") {
        server = process.env.SERVER_URL;
    }
    let nodeModulePath = "./node_modules";
    if (typeof process.env.NODE_MODULE_PATH !== "undefined") {
        nodeModulePath = process.env.NODE_MODULE_PATH;
    }

    const filesToTest = argv.slice(2).filter(v => !v.startsWith("--"));
    const options = argv.slice(2).filter(v => v.startsWith("--"));
    const cmd = [
        path.join(nodeModulePath, "browser-ui-test/src/index.js"),
        "--display-format",
        "compact",
        "--variable",
        "DOC_PATH",
        server,
    ];
    if (filesToTest.length === 0) {
        cmd.push("--test-folder");
        cmd.push(__dirname);
    } else {
        for (const fileToTest of filesToTest) {
            cmd.push("--test-file");
            cmd.push(fileToTest);
        }
    }
    for (const option of options) {
        cmd.push(option);
    }
    await spawn("node", cmd, {stdio: "inherit", stderr: "inherit"}).on("exit", code => {
        if (code !== 0) {
            process.exit(1);
        }
    });
}

main(process.argv);
