require 'administrate/base_dashboard'

class PaymentDashboard < Administrate::BaseDashboard
  # ATTRIBUTE_TYPES
  # a hash that describes the type of each of the model's fields.
  #
  # Each different type represents an Administrate::Field object,
  # which determines how the attribute is displayed
  # on pages throughout the dashboard.
  ATTRIBUTE_TYPES = {
    id: Field::Number,
    user: Field::BelongsTo.with_options(
      searchable: true,
      searchable_field: 'email'
    ),
    payment_id: Field::String,
    wire_transfer: Field::Boolean,
    subscription_plan: Field::BelongsTo,
    status: Field::String.with_options(searchable: false),
    external_statuses: Field::String,
    total: Field::String.with_options(searchable: false),
    currency: Field::String.with_options(searchable: false),
    crypto_total: Field::String.with_options(searchable: false),
    created_at: Field::DateTime,
    updated_at: Field::DateTime,
    first_name: Field::String,
    last_name: Field::String,
    birth_date: Field::DateTime.with_options(format: '%F'),
    country: Field::String,
    crypto_paid: Field::String.with_options(searchable: false),
    commission: Field::Number.with_options(searchable: false),
    crypto_commission: Field::Number.with_options(searchable: false),
    paid_at: Field::DateTime.with_options(format: '%F %r')
  }.freeze

  # COLLECTION_ATTRIBUTES
  # an array of attributes that will be displayed on the model's index page.
  #
  # By default, it's limited to four items to reduce clutter on index pages.
  # Feel free to add, remove, or rearrange items.
  COLLECTION_ATTRIBUTES = %i[
    id
    subscription_plan
    wire_transfer
    status
    total
    currency
    first_name
    last_name
    birth_date
    country
    crypto_paid
    paid_at
    user
    payment_id
  ].freeze

  # SHOW_PAGE_ATTRIBUTES
  # an array of attributes that will be displayed on the model's show page.
  SHOW_PAGE_ATTRIBUTES = %i[
    id
    subscription_plan
    wire_transfer
    status
    total
    currency
    first_name
    last_name
    birth_date
    country
    crypto_total
    crypto_paid
    paid_at
    user
    payment_id
    commission
    crypto_commission
    external_statuses
    created_at
    updated_at
  ].freeze

  # FORM_ATTRIBUTES
  # an array of attributes that will be displayed
  # on the model's form (`new` and `edit`) pages.
  FORM_ATTRIBUTES = %i[
    subscription_plan
    status
    total
    currency
    first_name
    last_name
    birth_date
    country
    crypto_total
    crypto_paid
    paid_at
    user
    payment_id
  ].freeze

  # COLLECTION_FILTERS
  # a hash that defines filters that can be used while searching via the search
  # field of the dashboard.
  #
  # For example to add an option to search for open resources by typing "open:"
  # in the search field:
  #
  COLLECTION_FILTERS = {
    paid: ->(resources) { resources.where(status: :paid) },
    unpaid: ->(resources) { resources.where.not(status: :paid) },
    wire: ->(resources) { resources.where(wire_transfer: true) }

  }.freeze
  # COLLECTION_FILTERS = {}.freeze

  # Overwrite this method to customize how payments are displayed
  # across all pages of the admin dashboard.
  #
  # def display_resource(payment)
  #   "Payment ##{payment.id}"
  # end
end
