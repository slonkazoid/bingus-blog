function replaceDates() {
	for (let el of document.querySelectorAll(".date-rfc3339")) {
		let date = new Date(Date.parse(el.textContent));
		el.textContent = date.toLocaleString();
		el.classList.remove("date-rfc3339");
	}
}
