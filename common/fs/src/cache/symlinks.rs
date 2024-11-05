use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct Symlinks(HashMap<PathBuf, Vec<PathBuf>>);

impl Symlinks {
    pub fn get(&self, target: &Path) -> Option<&Vec<PathBuf>> {
        self.0.get(target)
    }

    pub fn add(&mut self, symlink: PathBuf, target: PathBuf) {
        let symlinks = self.0.entry(target).or_default();
        symlinks.push(symlink)
    }

    pub fn remove(&mut self, symlink: &Path, target: &Path) {
        if let Some((index, symlinks)) = self
            .0
            .get_mut(target)
            .and_then(|ss| ss.iter().position(|s| *s == symlink).zip(Some(ss)))
        {
            symlinks.swap_remove(index);
            if symlinks.is_empty() {
                self.0.remove(target);
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_symlinks() {
        let symlink1 = PathBuf::from("/var/log/test1.log");
        let symlink2 = PathBuf::from("/var/log/test2.log");
        let symlink3 = PathBuf::from("/var/log/test3.log");
        let target = PathBuf::from("/var/lib/log/test.log");
        let mut symlinks = Symlinks::default();
        assert_eq!(symlinks.get(&target), None);

        {
            symlinks.add(symlink1.clone(), target.clone());
            let vec = symlinks.get(&target).unwrap();
            assert_eq!(*vec[0], symlink1);
            assert_eq!(vec.len(), 1);
        }
        {
            symlinks.add(symlink2.clone(), target.clone());
            let vec = symlinks.get(&target).unwrap();
            assert_eq!(*vec[0], symlink1);
            assert_eq!(*vec[1], symlink2);
            assert_eq!(vec.len(), 2);
        }
        {
            symlinks.add(symlink3.clone(), target.clone());
            let vec = symlinks.get(&target).unwrap();
            assert_eq!(*vec[0], symlink1);
            assert_eq!(*vec[1], symlink2);
            assert_eq!(*vec[2], symlink3);
            assert_eq!(vec.len(), 3);
        }
        {
            symlinks.remove(&symlink1, &target);
            let vec = symlinks.get(&target).unwrap();
            assert_eq!(*vec[0], symlink3);
            assert_eq!(*vec[1], symlink2);
            assert_eq!(vec.len(), 2);
        }
        {
            symlinks.remove(&symlink2, &target);
            let vec = symlinks.get(&target).unwrap();
            assert_eq!(*vec[0], symlink3);
            assert_eq!(vec.len(), 1);
        }
        {
            symlinks.remove(&symlink3, &target);
            assert_eq!(symlinks.get(&target), None);
        }
    }
}
