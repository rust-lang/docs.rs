(function() {
    const clipboard = document.getElementById("clipboard");
    if (clipboard) {
        let resetClipboardTimeout = null;
        let resetClipboardIcon = clipboard.innerHTML;

        function resetClipboard() {
            resetClipboardTimeout = null;
            clipboard.innerHTML = resetClipboardIcon;
        }

        async function copyTextHandler() {
            const metadata = JSON.parse(document.getElementById("crate-metadata").innerText)

            const temporaryInput = document.createElement("input");
            temporaryInput.type = "text";
            temporaryInput.value = `${metadata.name} = "${metadata.version}"`;

            document.body.append(temporaryInput);
            temporaryInput.select();
            document.execCommand("copy");
            temporaryInput.remove();

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
