(function() {
    const clipboard = document.getElementById("clipboard");
    if (clipboard) {
        let resetClipboardTimeout = null;
        const resetClipboardIcon = clipboard.innerHTML;

        clipboard.addEventListener("click", () => {
            const metadata = JSON.parse(document.getElementById("crate-metadata").innerText);

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
            resetClipboardTimeout = setTimeout(() => {
                resetClipboardTimeout = null;
                clipboard.innerHTML = resetClipboardIcon;
            }, 1000);
        });
    }

    for (const e of document.querySelectorAll("a[data-fragment=\"retain\"]")) {
        e.addEventListener("mouseover", () => {
            e.hash = document.location.hash;
        });
    }
})();
