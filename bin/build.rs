fn main() {
    #[cfg(feature = "dep_audit")]
    auditable_build::collect_dependency_list();
}
