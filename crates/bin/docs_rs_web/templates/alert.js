(function() {
    const alertCheckbox = document.getElementById("docsrs-alert-input");
    alertCheckbox.onchange = () => {
        // If the user clicks on the "close button", we save this info in their local storage.
        window.localStorage.setItem("hide-alert-id", alertCheckbox.getAttribute("data-id"));
    };

    const info = window.localStorage.getItem("hide-alert-id");
    if (info !== null) {
        const alertId = alertCheckbox.getAttribute("data-id");
        // If the user already "closed" the alert, we don't show it anymore.
        if (alertId === info) {
            alertCheckbox.checked = true;
        }
    }
})();
