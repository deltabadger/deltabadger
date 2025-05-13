require 'administrate/base_dashboard'

class TransactionDashboard < Administrate::BaseDashboard
  # ATTRIBUTE_TYPES
  # a hash that describes the type of each of the model's fields.
  #
  # Each different type represents an Administrate::Field object,
  # which determines how the attribute is displayed
  # on pages throughout the dashboard.
  ATTRIBUTE_TYPES = {
    bot: Field::BelongsTo,
    id: Field::Number,
    external_id: Field::String.with_options(searchable: false),
    rate: Field::String.with_options(searchable: false),
    amount: Field::String.with_options(searchable: false),
    status: Field::String.with_options(searchable: false),
    bot_price: Field::String.with_options(searchable: false),
    bot_interval: Field::String.with_options(searchable: false),
    created_at: Field::DateTime,
    updated_at: Field::DateTime,
    error_messages: Field::String.with_options(searchable: false),
    base: Field::String.with_options(searchable: false),
    quote: Field::String.with_options(searchable: false),
    exchange: Field::BelongsTo
  }.freeze

  # COLLECTION_ATTRIBUTES
  # an array of attributes that will be displayed on the model's index page.
  #
  # By default, it's limited to four items to reduce clutter on index pages.
  # Feel free to add, remove, or rearrange items.
  COLLECTION_ATTRIBUTES = %i[
    bot
    id
    external_id
    rate
    amount
    exchange
  ].freeze

  # SHOW_PAGE_ATTRIBUTES
  # an array of attributes that will be displayed on the model's show page.
  SHOW_PAGE_ATTRIBUTES = %i[
    id
    external_id
    bot
    exchange
    rate
    amount
    status
    bot_price
    bot_interval
    error_messages
    base
    quote
    created_at
    updated_at
  ].freeze

  # FORM_ATTRIBUTES
  # an array of attributes that will be displayed
  # on the model's form (`new` and `edit`) pages.
  FORM_ATTRIBUTES = %i[
    bot
    external_id
    rate
    amount
    market
    status
    currency
    error_messages
    base
    quote
    exchange
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

  # Overwrite this method to customize how transactions are displayed
  # across all pages of the admin dashboard.
  #
  # def display_resource(transaction)
  #   "Transaction ##{transaction.id}"
  # end
end
