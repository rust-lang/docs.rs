(function() {
    // Delegated, because the rustdoc crate dropdown is loaded later.
    let resetClipboardTimeout = null;
    let resetClipboardIcon = null;
    document.addEventListener("click", ev => {
        const clipboard = ev.target.closest?.("#clipboard");
        if (!clipboard) {
            return;
        }
        const metadata = JSON.parse(document.getElementById("crate-metadata").innerText);

        const temporaryInput = document.createElement("input");
        temporaryInput.type = "text";
        temporaryInput.value = `${metadata.name} = "${metadata.version}"`;

        document.body.append(temporaryInput);
        temporaryInput.select();
        document.execCommand("copy");
        temporaryInput.remove();

        if (resetClipboardTimeout === null) {
            resetClipboardIcon = clipboard.innerHTML;
        } else {
            clearTimeout(resetClipboardTimeout);
        }
        clipboard.textContent = "✓";
        resetClipboardTimeout = setTimeout(() => {
            resetClipboardTimeout = null;
            clipboard.innerHTML = resetClipboardIcon;
        }, 1000);
    });

    document.addEventListener("mouseover", ev => {
        const link = ev.target.closest?.("a[data-fragment=\"retain\"]");
        if (link) {
            link.hash = document.location.hash;
        }
    });
})();
