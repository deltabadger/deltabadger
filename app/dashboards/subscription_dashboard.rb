require 'administrate/base_dashboard'

class SubscriptionDashboard < Administrate::BaseDashboard
  # ATTRIBUTE_TYPES
  # a hash that describes the type of each of the model's fields.
  #
  # Each different type represents an Administrate::Field object,
  # which determines how the attribute is displayed
  # on pages throughout the dashboard.
  ATTRIBUTE_TYPES = {
    subscription_plan: Field::BelongsTo,
    user: Field::BelongsTo,
    id: Field::Number,
    end_time: Field::DateTime.with_options(format: '%F'),
    created_at: Field::DateTime,
    updated_at: Field::DateTime,
    credits: Field::String.with_options(searchable: false),
    limit_almost_reached_sent: Field::Boolean,
    sequence_number: Field::Number
  }.freeze

  # COLLECTION_ATTRIBUTES
  # an array of attributes that will be displayed on the model's index page.
  #
  # By default, it's limited to four items to reduce clutter on index pages.
  # Feel free to add, remove, or rearrange items.
  COLLECTION_ATTRIBUTES = %i[
    id
    subscription_plan
    user
    end_time
    sequence_number
  ].freeze

  # SHOW_PAGE_ATTRIBUTES
  # an array of attributes that will be displayed on the model's show page.
  SHOW_PAGE_ATTRIBUTES = %i[
    subscription_plan
    user
    id
    end_time
    created_at
    updated_at
    credits
    limit_almost_reached_sent
    sequence_number
  ].freeze

  # FORM_ATTRIBUTES
  # an array of attributes that will be displayed
  # on the model's form (`new` and `edit`) pages.
  FORM_ATTRIBUTES = %i[
    subscription_plan
    user
    end_time
    credits
    limit_almost_reached_sent
    sequence_number
  ].freeze

  # COLLECTION_FILTERS
  # a hash that defines filters that can be used while searching via the search
  # field of the dashboard.
  #
  # For example to add an option to search for open resources by typing "open:"
  # in the search field:
  #
  COLLECTION_FILTERS = {
    current: ->(_resources) { where('end_time > ?', Time.now) },
    old: ->(_resources) { where('end_time < ?', Time.now) }
  }.freeze
  # COLLECTION_FILTERS = {}.freeze

  # Overwrite this method to customize how subscriptions are displayed
  # across all pages of the admin dashboard.
  #
  # def display_resource(subscription)
  #   "Subscription ##{subscription.id}"
  # end
end
