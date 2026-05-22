require "reject";
if header :contains "From" "spam@" {
    reject "Spam not welcome";
}
