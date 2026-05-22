require "fileinto";
if header :contains "Subject" "Invoice" {
    fileinto "Finance";
}
