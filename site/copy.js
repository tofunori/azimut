// Adds a "copy" button to install commands; the page works without it.
document.querySelectorAll(".cmd button").forEach(function (button) {
  var label = button.textContent;
  var done = document.documentElement.lang === "fr" ? "Copié" : "Copied";
  button.hidden = !navigator.clipboard;
  button.addEventListener("click", function () {
    var text = button.parentElement.querySelector("code").textContent;
    navigator.clipboard.writeText(text).then(function () {
      button.textContent = done;
      setTimeout(function () { button.textContent = label; }, 1500);
    });
  });
});
