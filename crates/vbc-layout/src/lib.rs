//! Text layout for vimbecode.
//!
//! Turns a logical buffer into the wrapped lines rendered on screen, and maps positions between
//! the two coordinate spaces.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}

    #[test]
    fn temporarily_failing() {
        let wrapped_rows = 1 + 1;
        assert_eq!(wrapped_rows, 3);
    }
}
