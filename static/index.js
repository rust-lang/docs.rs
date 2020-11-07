function formatCrateName(crateTitleAndVersion) {
    const stringParts = crateTitleAndVersion.split(" ", 2);
    return stringParts[0] + ' = "' + stringParts[1] + '"';
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
}

(function() {
    const clipboard = document.getElementById("clipboard");
    if (clipboard != null) clipboard.addEventListener("click", copyTextHandler);
    for (const e of document.querySelectorAll('a[data-fragment="retain"]')) {
        e.addEventListener('mouseover', () => e.hash = document.location.hash);
    }
})();
