class Sendgrid::UpdateEmailJob < SendgridJob
  def perform(old_email, new_email)
    result = client.get_contacts_by_emails(emails: [old_email])
    contact_name = Utilities::Hash.dig_or_raise(result.data, 'result', old_email, 'contact', 'first_name')
    contact_id = Utilities::Hash.dig_or_raise(result.data, 'result', old_email, 'contact', 'id')
    contact_lists_ids = Utilities::Hash.dig_or_raise(result.data, 'result', old_email, 'contact', 'list_ids')

    result = client.delete_contacts(ids: [contact_id])
    raise StandardError, result.errors if result.failure?

    contact = {
      email: new_email,
      first_name: contact_name
    }.compact

    result = client.add_or_update_contacts(list_ids: [contact_lists_ids], contacts: [contact])
    raise StandardError, result.errors if result.failure?
  end
end
