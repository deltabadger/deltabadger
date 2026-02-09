# frozen_string_literal: true

# Pagy initializer file (43.x)
# Pagy 43+ uses autoloading — no more `require 'pagy/extras/*'`.
# Use `pagy(:countless, collection)` instead of `pagy_countless(collection)`.

# Use the standard i18n gem for translations (needed for Rails locale integration)
Pagy.translate_with_the_slower_i18n_gem!
