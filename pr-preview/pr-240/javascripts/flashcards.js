/* Flashcard flip on click/tap */
document$.subscribe(function () {
  document.querySelectorAll(".flashcard").forEach(function (card) {
    // Avoid double-binding on navigation
    if (card.dataset.fcBound) return;
    card.dataset.fcBound = "1";
    card.addEventListener("click", function () {
      card.classList.toggle("flipped");
    });
    // Keyboard support
    card.setAttribute("tabindex", "0");
    card.setAttribute("role", "button");
    card.setAttribute("aria-label", "Flashcard — click to flip");
    card.addEventListener("keydown", function (e) {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        card.classList.toggle("flipped");
      }
    });
  });
});
