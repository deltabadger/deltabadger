require 'administrate/base_dashboard'

class AffiliateDashboard < Administrate::BaseDashboard
  # ATTRIBUTE_TYPES
  # a hash that describes the type of each of the model's fields.
  #
  # Each different type represents an Administrate::Field object,
  # which determines how the attribute is displayed
  # on pages throughout the dashboard.
  ATTRIBUTE_TYPES = {
    user: Field::BelongsTo,
    id: Field::Number,
    type: Field::String,
    active: Field::Boolean,
    name: Field::String,
    address: Field::String,
    vat_number: Field::String,
    btc_address: Field::String,
    code: Field::String,
    visible_name: Field::String,
    visible_link_scheme: Field::String,
    visible_link: Field::String,
    discount_percent: Field::Number,
    total_bonus_percent: Field::Number,
    unexported_btc_commission: Field::Number,
    exported_btc_commission: Field::Number,
    paid_btc_commission: Field::Number,
    created_at: Field::DateTime,
    updated_at: Field::DateTime
  }.freeze

  # COLLECTION_ATTRIBUTES
  # an array of attributes that will be displayed on the model's index page.
  #
  # By default, it's limited to four items to reduce clutter on index pages.
  # Feel free to add, remove, or rearrange items.
  COLLECTION_ATTRIBUTES = %i[
    user
    id
    type
    active
    btc_address
    code
  ].freeze

  # SHOW_PAGE_ATTRIBUTES
  # an array of attributes that will be displayed on the model's show page.
  SHOW_PAGE_ATTRIBUTES = %i[
    user
    id
    type
    active
    name
    address
    vat_number
    btc_address
    code
    visible_name
    visible_link_scheme
    visible_link
    discount_percent
    total_bonus_percent
    unexported_btc_commission
    exported_btc_commission
    paid_btc_commission
    created_at
    updated_at
  ].freeze

  # FORM_ATTRIBUTES
  # an array of attributes that will be displayed
  # on the model's form (`new` and `edit`) pages.
  FORM_ATTRIBUTES = %i[
    user
    active
    type
    name
    address
    vat_number
    btc_address
    code
    visible_name
    visible_link_scheme
    visible_link
    discount_percent
    total_bonus_percent
    unexported_btc_commission
    exported_btc_commission
    paid_btc_commission
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
  COLLECTION_FILTERS = {
    active: ->(resources) { resources.active },
    inactive: ->(resources) { resources.inactive }
  }.freeze

  # Overwrite this method to customize how affiliates are displayed
  # across all pages of the admin dashboard.
  #
  # def display_resource(affiliate)
  #   "Affiliate ##{affiliate.id}"
  # end
end
