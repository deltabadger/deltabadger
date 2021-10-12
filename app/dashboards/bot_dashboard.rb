require 'administrate/base_dashboard'

class BotDashboard < Administrate::BaseDashboard
  # ATTRIBUTE_TYPES
  # a hash that describes the type of each of the model's fields.
  #
  # Each different type represents an Administrate::Field object,
  # which determines how the attribute is displayed
  # on pages throughout the dashboard.
  ATTRIBUTE_TYPES = {
    exchange: Field::BelongsTo,
    user: Field::BelongsTo,
    transactions: Field::HasMany.with_options(sort_by: :id, direction: :desc),
    id: Field::Number,
    status: Field::String.with_options(searchable: false),
    settings: Field::String.with_options(searchable: false),
    created_at: Field::DateTime,
    updated_at: Field::DateTime,
    bot_type: Field::String.with_options(searchable: false),
    type: Field::String,
    interval: Field::String,
    price: Field::String,
    total_amount: Field::Number
  }.freeze

  # COLLECTION_ATTRIBUTES
  # an array of attributes that will be displayed on the model's index page.
  #
  # By default, it's limited to four items to reduce clutter on index pages.
  # Feel free to add, remove, or rearrange items.
  COLLECTION_ATTRIBUTES = %i[
    id
    status
    exchange
    price
    interval
    type
    transactions
    total_amount
  ].freeze

  # SHOW_PAGE_ATTRIBUTES
  # an array of attributes that will be displayed on the model's show page.
  SHOW_PAGE_ATTRIBUTES = %i[
    exchange
    user
    transactions
    id
    status
    settings
    created_at
    updated_at
    bot_type
  ].freeze

  # FORM_ATTRIBUTES
  # an array of attributes that will be displayed
  # on the model's form (`new` and `edit`) pages.
  FORM_ATTRIBUTES = %i[
    exchange
    status
    settings
    bot_type
  ].freeze

  # COLLECTION_FILTERS
  # a hash that defines filters that can be used while searching via the search
  # field of the dashboard.
  #
  # For example to add an option to search for open resources by typing "open:"
  # in the search field:
  #
  #   COLLECTION_FILTERS = {
  #     open: ->(resources) { where(open: true) }
  #   }.freeze
  COLLECTION_FILTERS = {}.freeze

  # Overwrite this method to customize how bots are displayed
  # across all pages of the admin dashboard.
  #
  # def display_resource(bot)
  #   "Bot ##{bot.id}"
  # end
end
