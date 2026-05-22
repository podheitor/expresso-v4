require ["fileinto", "reject"];
if allof (
    header :contains "Subject" "URGENT",
    header :contains "From" "boss@"
) {
    fileinto "Priority";
} elsif header :contains "Subject" "SPAM" {
    reject "Unwanted message";
} else {
    keep;
}