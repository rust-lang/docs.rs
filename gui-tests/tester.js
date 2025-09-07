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
    await spawn("node", [
        path.join(nodeModulePath, "browser-ui-test/src/index.js"),
        "--display-format",
        "compact",
        "--variable",
        "DOC_PATH",
        server,
        "--test-folder",
        __dirname,
        ...argv.slice(2),
    ], {stdio: "inherit", stderr: "inherit"}).on("exit", code => {
        if (code !== 0) {
            process.exit(1);
        }
    });
}

main(process.argv);
