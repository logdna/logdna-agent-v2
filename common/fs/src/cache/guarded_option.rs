use std::ops::Deref;

#[derive(Debug, Clone)]
pub struct GuardedOption<T> {
    value: T,
    state: bool,
}

impl<'a, T> GuardedOption<T> {
    pub fn new(value: T, state: bool) -> Self {
        Self { value, state }
    }

    pub fn toggle_on(&mut self) {
        self.state = true;
    }

    pub fn toggle_off(&mut self) {
        self.state = false;
    }

    pub fn toggle(&mut self) {
        self.state ^= true;
    }

    pub fn as_value(&'a self) -> &'a T {
        &self.value
    }

    pub fn to_option(&self) -> Option<T>
    where
        T: Clone,
    {
        if self.state {
            Some(self.value.clone())
        } else {
            None
        }
    }

    pub fn as_option(&'a self) -> Option<&'a T> {
        if self.state {
            Some(&self.value)
        } else {
            None
        }
    }

    pub fn as_option_deref(&self) -> Option<&T::Target>
    where
        T: Deref,
    {
        if self.state {
            Some(self.value.deref())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_guarded_option() {
        let string = String::from("hello");
        let mut guarded_option = GuardedOption::new(string.clone(), false);
        assert_eq!(*guarded_option.as_value(), string);
        assert_eq!(guarded_option.to_option(), None);
        assert_eq!(guarded_option.as_option(), None);
        assert_eq!(guarded_option.as_option_deref(), None);

        guarded_option.toggle_off();
        assert_eq!(guarded_option.to_option(), None);
        assert_eq!(guarded_option.as_option(), None);
        assert_eq!(guarded_option.as_option_deref(), None);

        guarded_option.toggle_on();
        assert_eq!(guarded_option.to_option(), Some(string.clone()));
        assert_eq!(guarded_option.as_option(), Some(&string));
        assert_eq!(guarded_option.as_option_deref(), Some(string.as_str()));

        guarded_option.toggle();
        assert_eq!(guarded_option.to_option(), None);
        assert_eq!(guarded_option.as_option(), None);
        assert_eq!(guarded_option.as_option_deref(), None);

        guarded_option.toggle();
        assert_eq!(guarded_option.to_option(), Some(string.clone()));
        assert_eq!(guarded_option.as_option(), Some(&string));
        assert_eq!(guarded_option.as_option_deref(), Some(string.as_str()));
    }
}
