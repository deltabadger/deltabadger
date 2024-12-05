require 'utilities/hash'

class Sendgrid::RemoveEmailFromListJob < SendgridJob
  def perform(email, list_name)
    list_id = get_list_id(list_name)
    raise StandardError, "List not found: #{list_name}" if list_id.nil?

    result = client.get_contacts_by_emails(emails: [email])
    contact_id = Utilities::Hash.dig_or_raise(result.data, 'result', email, 'contact', 'id')

    result = client.remove_contacts_from_list(id: list_id, contact_ids: [contact_id])
    raise StandardError, result.errors if result.failure?
  end
end
