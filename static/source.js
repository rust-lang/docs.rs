(function() {
    let oldLabel;

    function showSourceFiles(button, sideMenu, sourceCode) {
        button.title = oldLabel;
        button.setAttribute("aria-label", button.title);
        button.setAttribute("aria-expanded", "true");

        sideMenu.classList.remove("collapsed");
        sourceCode.classList.remove("expanded");
    }

    function hideSourceFiles(button, sideMenu, sourceCode) {
        button.title = "Show source sidebar";
        button.setAttribute("aria-label", button.title);
        button.setAttribute("aria-expanded", "false");

        sideMenu.classList.add("collapsed");
        sourceCode.classList.add("expanded");
    }

    function toggleSource(button) {
        const sideMenu = document.getElementById("side-menu");
        const sourceCode = document.getElementById("source-code");

        if (sideMenu.classList.contains("collapsed")) {
            showSourceFiles(button, sideMenu, sourceCode);
        } else {
            hideSourceFiles(button, sideMenu, sourceCode);
        }
    }

    document.addEventListener("DOMContentLoaded", () => {
        const toggleSourceButton = document.querySelector("li.toggle-source button");
        oldLabel = toggleSourceButton.getAttribute("aria-label");

        toggleSourceButton.addEventListener("click", () => {
            toggleSource(toggleSourceButton);
        });
    });

    // This code has been adapted from the rustdoc implementation here:
    // https://github.com/rust-lang/rust/blob/5c848860/src/librustdoc/html/static/js/src-script.js#L152-L204
    function highlightLineNumbers() {
        const match = window.location.hash.match(/^#?(\d+)(?:-(\d+))?$/);
        if (!match) {
            return;
        }
        let from = parseInt(match[1], 10);
        let to = from;
        if (typeof match[2] !== "undefined") {
            to = parseInt(match[2], 10);
        }
        if (to < from) {
            const tmp = to;
            to = from;
            from = tmp;
        }
        let elem = document.getElementById(from);
        if (!elem) {
            return;
        }
        const x = document.getElementById(from);
        if (x) {
            x.scrollIntoView();
        }
        Array.from(document.getElementsByClassName("line-number-highlighted")).forEach(e => {
            e.classList.remove("line-number-highlighted");
        });
        for (let i = from; i <= to; ++i) {
            elem = document.getElementById(i);
            if (!elem) {
                break;
            }
            elem.classList.add("line-number-highlighted");
        }
    }

    const handleLineNumbers = (function () {
        let prev_line_id = 0;

        const set_fragment = name => {
            const x = window.scrollX,
                y = window.scrollY;
            if (window.history && typeof window.history.pushState === "function") {
                history.replaceState(null, null, "#" + name);
                highlightLineNumbers();
            } else {
                location.replace("#" + name);
            }
            // Prevent jumps when selecting one or many lines
            window.scrollTo(x, y);
        };

        return ev => {
            let cur_line_id = parseInt(ev.target.id, 10);
            // This event handler is attached to the entire line number column, but it should only
            // be run if one of the anchors is clicked. It also shouldn't do anything if the anchor
            // is clicked with a modifier key (to open a new browser tab).
            if (isNaN(cur_line_id) ||
                ev.ctrlKey ||
                ev.altKey ||
                ev.metaKey) {
                return;
            }
            ev.preventDefault();

            if (ev.shiftKey && prev_line_id) {
                // Swap selection if needed
                if (prev_line_id > cur_line_id) {
                    const tmp = prev_line_id;
                    prev_line_id = cur_line_id;
                    cur_line_id = tmp;
                }

                set_fragment(prev_line_id + "-" + cur_line_id);
            } else {
                prev_line_id = cur_line_id;

                set_fragment(cur_line_id);
            }
        };
    }());

    window.addEventListener("hashchange", highlightLineNumbers)

    Array.from(document.getElementById("line-numbers").children[0].children).forEach(el => {
        el.addEventListener("click", handleLineNumbers);
    });

    highlightLineNumbers();
})();
