require "fileinto";
if header :contains "Subject" {
    fileinto
}