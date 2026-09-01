use std::collections::{HashMap, VecDeque};
use std::mem::take;

use futures::FutureExt;
use gpui::{
    App, AppContext, Asset, AssetLogger, ElementId, Entity, ImageAssetLoader, ImageCache,
    ImageCacheItem, ImageCacheProvider, ImageSource, Resource, hash,
};

/// Default number of images each view's cache retains. Loading a new image
/// evicts the least recently used entry once this is reached.
pub const MAX_IMAGES: usize = 128;

pub fn image_cache(id: impl Into<ElementId>, max_items: usize) -> AppImageCacheProvider {
    AppImageCacheProvider {
        id: id.into(),
        max_items,
    }
}

pub struct AppImageCacheProvider {
    id: ElementId,
    max_items: usize,
}

impl ImageCacheProvider for AppImageCacheProvider {
    fn provide(&mut self, window: &mut gpui::Window, cx: &mut App) -> gpui::AnyImageCache {
        window
            .with_global_id(self.id.clone(), |id, window| {
                window.with_element_state(id, |cache, _| {
                    let cache = cache.unwrap_or_else(|| AppImageCache::new(self.max_items, cx));
                    (cache.clone(), cache)
                })
            })
            .into()
    }
}

pub struct AppImageCache {
    max_items: usize,
    usage_list: VecDeque<u64>,
    cache: HashMap<u64, (ImageCacheItem, Resource)>,
}

impl AppImageCache {
    pub fn new(max_items: usize, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            log::info!("Creating AppImageCacheProvider");
            cx.on_release(|this: &mut Self, cx| {
                for (ix, (mut image, resource)) in take(&mut this.cache) {
                    if let Some(Ok(image)) = image.get() {
                        log::info!("Dropping image {ix}");
                        cx.drop_image(image, None);
                    }
                    ImageSource::Resource(resource).remove_asset(cx);
                }
            })
            .detach();

            AppImageCache {
                max_items,
                usage_list: VecDeque::with_capacity(max_items),
                cache: HashMap::with_capacity(max_items),
            }
        })
    }
}

impl ImageCache for AppImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<Result<std::sync::Arc<gpui::RenderImage>, gpui::ImageCacheError>> {
        let hash = hash(resource);

        if let Some(item) = self.cache.get_mut(&hash) {
            let current_idx = self
                .usage_list
                .iter()
                .position(|item| *item == hash)
                .expect("cache has an item usage_list doesn't");

            self.usage_list.remove(current_idx);
            self.usage_list.push_front(hash);

            return item.0.get();
        }

        let load_future = AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx);
        let task = cx.background_executor().spawn(load_future).shared();

        if self.usage_list.len() >= self.max_items {
            log::info!("Image cache is full, evicting oldest item");

            if let Some(oldest) = self.usage_list.pop_back() {
                let mut image = self
                    .cache
                    .remove(&oldest)
                    .expect("usage_list has an item cache doesn't");

                if let Some(Ok(image)) = image.0.get() {
                    log::info!("requesting image to be dropped");
                    cx.drop_image(image, Some(window));
                }

                ImageSource::Resource(image.1).remove_asset(cx);
            }
        }

        self.cache.insert(
            hash,
            (
                gpui::ImageCacheItem::Loading(task.clone()),
                resource.clone(),
            ),
        );
        self.usage_list.push_front(hash);

        let entity = window.current_view();

        window
            .spawn(cx, async move |cx| {
                let result = task.await;

                if let Err(err) = result {
                    log::error!("error loading image into cache: {:?}", err);
                }

                cx.on_next_frame(move |_, cx| {
                    cx.notify(entity);
                });
            })
            .detach();

        None
    }
}
