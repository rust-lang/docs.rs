function formatCrateName(crateTitleAndVersion) {
    const stringParts = crateTitleAndVersion.split(" ", 2);
    return stringParts[0] + ' = "' + stringParts[1] + '"';
}

function copyCode() {
    let preElements = document.querySelectorAll('pre');
    preElements.forEach(function(pre, index) {
        let resetTimeout = null;
        let icon = "<img src='/-/rustdoc.static/clipboard-7571035ce49a181d.svg' width='15' height='15'>";
        let copyBtn = document.createElement("button");
        copyBtn.classList.add("copy-btn-" + index);
        copyBtn.style.width = "32px";
        copyBtn.style.height = "30px";
        copyBtn.style.float = "right";
        copyBtn.style.opacity = "0";
        copyBtn.style.transition = "ease 0.3s";
        copyBtn.style.filter = "invert(50%)";
        copyBtn.style.background = "transparent";
        copyBtn.style.border = "1px solid var(--main-color)";
        copyBtn.style.borderRadius = "5px";
        copyBtn.style.padding = "5px 7px 4px 8px";
        copyBtn.ariaLabel = "Copy to clipboard";
        copyBtn.innerHTML = icon;
        pre.prepend(copyBtn);
        pre.style.borderRadius = "10px";
        pre.addEventListener("mouseenter", function() {
            copyBtn.style.opacity = "1";
            copyBtn.addEventListener('click', function() {
                let code = pre.querySelector("code").textContent;
                navigator.clipboard.writeText(code).then(function() {
                    copyBtn.textContent = "✓";
                if (resetTimeout !== null) {
                    clearTimeout(resetTimeout);
                }
                    resetTimeout = setTimeout(function() {
                        copyBtn.innerHTML = icon;
                    }, 1000);
                    console.log("copied");
                    
                });
            });
        });
        pre.addEventListener("mouseleave", function() {
            copyBtn.style.opacity = "0";
        });
    });
}

(function() {
    copyCode();
    const clipboard = document.getElementById("clipboard");
    if (clipboard) {
        let resetClipboardTimeout = null;
        let resetClipboardIcon = clipboard.innerHTML;

        function resetClipboard() {
            resetClipboardTimeout = null;
            clipboard.innerHTML = resetClipboardIcon;
        }

        function copyTextHandler() {
            const crateTitleAndVersion = document.getElementById("crate-title");
            // On rustdoc pages, we use `textTransform: uppercase`, which copies as uppercase.
            // To avoid that, reset the styles temporarily.
            const oldTransform = crateTitleAndVersion.style.textTransform;
            crateTitleAndVersion.style.textTransform = "none";
            const temporaryInput = document.createElement("input");

            temporaryInput.type = "text";
            temporaryInput.value = formatCrateName(crateTitleAndVersion.innerText);

            document.body.append(temporaryInput);
            temporaryInput.select();
            document.execCommand("copy");

            temporaryInput.remove();
            crateTitleAndVersion.style.textTransform = oldTransform;

            clipboard.textContent = "✓";
            if (resetClipboardTimeout !== null) {
                clearTimeout(resetClipboardTimeout);
            }
            resetClipboardTimeout = setTimeout(resetClipboard, 1000);
        }

        clipboard.addEventListener("click", copyTextHandler);
    }
    for (const e of document.querySelectorAll('a[data-fragment="retain"]')) {
        e.addEventListener('mouseover', () => e.hash = document.location.hash);
    }
})();
