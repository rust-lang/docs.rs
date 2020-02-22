function formatCrateName(crateTitleAndVersion) {
    const stringParts = crateTitleAndVersion.split(" ", 2);
    return stringParts[0] + ' = "' + stringParts[1] + '"';
}

function copyTextHandler() {
    const crateTitleAndVersion = document.getElementById("crate-title").innerText;
    const temporaryInput = document.createElement("input");

    temporaryInput.type = "text";
    temporaryInput.value = formatCrateName(crateTitleAndVersion);

    document.body.append(temporaryInput);
    temporaryInput.select();
    document.execCommand("copy");

    temporaryInput.remove();
}

document.getElementById("clipboard").addEventListener("click", copyTextHandler);
